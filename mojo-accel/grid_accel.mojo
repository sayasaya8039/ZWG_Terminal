# grid_accel.mojo — グリッドレンダラ用 SIMD 高速化モジュール
#
# ダメージスパン重複判定、セルバッファ pack/unpack、
# グリフキャッシュキー並列検索を SIMD で高速化する。

from sys.info import simdwidthof
from memory import UnsafePointer


# ── ダメージスパン重複判定 SIMD 化 ────────────────────────────────
# DamageSpan (start_col, end_col) の配列に対し、
# 指定カラム範囲との重複をSIMDで一括判定する。

def check_damage_overlap(
    span_starts_ptr: UnsafePointer[UInt16],
    span_ends_ptr: UnsafePointer[UInt16],
    span_count: Int,
    query_start: UInt16,
    query_end: UInt16,
) -> Bool:
    """ダメージスパン配列と指定カラム範囲の重複を SIMD で一括判定。

    重複条件: query_start < span.end_col AND span.start_col < query_end
    戻り値: 1つでも重複があれば True。
    """
    alias simd_width = simdwidthof[DType.uint16]()

    var q_start_vec = SIMD[DType.uint16, simd_width](query_start)
    var q_end_vec = SIMD[DType.uint16, simd_width](query_end)
    var pos: Int = 0

    # SIMD 幅ずつ一括チェック
    while pos + simd_width <= span_count:
        var starts = (span_starts_ptr + pos).load[width=simd_width]()
        var ends = (span_ends_ptr + pos).load[width=simd_width]()

        # 重複条件: query_start < ends AND starts < query_end
        var cond1 = q_start_vec < ends
        var cond2 = starts < q_end_vec
        var overlaps = cond1 & cond2

        if overlaps.reduce_or():
            return True

        pos += simd_width

    # 残りのスパンをスカラー処理
    while pos < span_count:
        var s = (span_starts_ptr + pos).load()
        var e = (span_ends_ptr + pos).load()
        if query_start < e and s < query_end:
            return True
        pos += 1

    return False


# ── セルバッファ pack (RGB + flags → u64) ─────────────────────────
# GridCell の (fg_rgb: u32, bg_rgb: u32, flags: u8) を
# 1つの u64 にパックする。GPU バッファ書き込み用。
#
# レイアウト: [flags:8][reserved:8][bg_rgb:24][fg_rgb:24]
#             = 64 bit

def pack_cell_attributes(
    fg_ptr: UnsafePointer[UInt32],
    bg_ptr: UnsafePointer[UInt32],
    flags_ptr: UnsafePointer[UInt8],
    out_ptr: UnsafePointer[UInt64],
    count: Int,
):
    """セル属性を u64 にパック (SIMD ベクトル化ヒント付き)。

    レイアウト: [flags:8][0:8][bg_rgb:24][fg_rgb:24]
    """
    # 現在の Mojo では u64 SIMD 幅が限られるため、
    # ループアンロールでベクトル化を促進
    var i: Int = 0

    # 4要素ずつアンロール
    while i + 4 <= count:
        for offset in range(4):
            var fg = (fg_ptr + i + offset).load().cast[DType.uint64]()
            var bg = (bg_ptr + i + offset).load().cast[DType.uint64]()
            var flags = (flags_ptr + i + offset).load().cast[DType.uint64]()

            # パック: fg を下位24bit、bg を次の24bit、flags を上位8bit
            var packed = (fg & 0xFFFFFF) | ((bg & 0xFFFFFF) << 24) | (flags << 56)
            (out_ptr + i + offset).store(packed)

        i += 4

    # 残り
    while i < count:
        var fg = (fg_ptr + i).load().cast[DType.uint64]()
        var bg = (bg_ptr + i).load().cast[DType.uint64]()
        var flags = (flags_ptr + i).load().cast[DType.uint64]()
        var packed = (fg & 0xFFFFFF) | ((bg & 0xFFFFFF) << 24) | (flags << 56)
        (out_ptr + i).store(packed)
        i += 1


# ── セルバッファ unpack (u64 → RGB + flags) ───────────────────────

def unpack_cell_attributes(
    packed_ptr: UnsafePointer[UInt64],
    fg_ptr: UnsafePointer[UInt32],
    bg_ptr: UnsafePointer[UInt32],
    flags_ptr: UnsafePointer[UInt8],
    count: Int,
):
    """パック済み u64 をセル属性に展開。"""
    var i: Int = 0

    while i + 4 <= count:
        for offset in range(4):
            var packed = (packed_ptr + i + offset).load()
            var fg = (packed & 0xFFFFFF).cast[DType.uint32]()
            var bg = ((packed >> 24) & 0xFFFFFF).cast[DType.uint32]()
            var flags = ((packed >> 56) & 0xFF).cast[DType.uint8]()
            (fg_ptr + i + offset).store(fg)
            (bg_ptr + i + offset).store(bg)
            (flags_ptr + i + offset).store(flags)

        i += 4

    while i < count:
        var packed = (packed_ptr + i).load()
        (fg_ptr + i).store((packed & 0xFFFFFF).cast[DType.uint32]())
        (bg_ptr + i).store(((packed >> 24) & 0xFFFFFF).cast[DType.uint32]())
        (flags_ptr + i).store(((packed >> 56) & 0xFF).cast[DType.uint8]())
        i += 1


# ── グリフキャッシュキー (u64) 並列検索 ───────────────────────────
# グリフキャッシュのキーハッシュ (u64) 配列から、
# 指定キーを SIMD で並列検索する。

def find_glyph_key(
    keys_ptr: UnsafePointer[UInt64],
    key_count: Int,
    target_key: UInt64,
) -> Int:
    """u64 グリフキャッシュキー配列から target_key を SIMD で並列検索。

    戻り値: 見つかったインデックス。見つからなければ -1。
    """
    alias simd_width = simdwidthof[DType.uint64]()

    var target_vec = SIMD[DType.uint64, simd_width](target_key)
    var pos: Int = 0

    while pos + simd_width <= key_count:
        var chunk = (keys_ptr + pos).load[width=simd_width]()
        var matches = chunk == target_vec

        if matches.reduce_or():
            for i in range(simd_width):
                if (keys_ptr + pos + i).load() == target_key:
                    return pos + i

        pos += simd_width

    # 残りをスカラー検索
    while pos < key_count:
        if (keys_ptr + pos).load() == target_key:
            return pos
        pos += 1

    return -1


# ── メモリコピー高速化 (GPU バッファ書き込み用) ────────────────────

def simd_memcpy_u8(
    dst_ptr: UnsafePointer[UInt8],
    src_ptr: UnsafePointer[UInt8],
    byte_count: Int,
):
    """SIMD 幅で一括コピー。GPU バッファ書き込みの高速化用。"""
    alias simd_width = simdwidthof[DType.uint8]()

    var pos: Int = 0

    while pos + simd_width <= byte_count:
        var chunk = (src_ptr + pos).load[width=simd_width]()
        (dst_ptr + pos).store(chunk)
        pos += simd_width

    # 残りバイト
    while pos < byte_count:
        (dst_ptr + pos).store((src_ptr + pos).load())
        pos += 1


# ── ダメージスパンマージ (ソート済み前提) ──────────────────────────

def merge_sorted_spans(
    starts_ptr: UnsafePointer[UInt16],
    ends_ptr: UnsafePointer[UInt16],
    span_count: Int,
    out_starts_ptr: UnsafePointer[UInt16],
    out_ends_ptr: UnsafePointer[UInt16],
) -> Int:
    """ソート済みダメージスパンを線形マージ。

    隣接・重複するスパンを結合して出力バッファに書き込む。
    戻り値: マージ後のスパン数。
    """
    if span_count == 0:
        return 0

    var out_count: Int = 0
    var cur_start = (starts_ptr + 0).load()
    var cur_end = (ends_ptr + 0).load()

    for i in range(1, span_count):
        var next_start = (starts_ptr + i).load()
        var next_end = (ends_ptr + i).load()

        if next_start <= cur_end:
            # 重複・隣接 → マージ
            if next_end > cur_end:
                cur_end = next_end
        else:
            # 新しいスパンを出力
            (out_starts_ptr + out_count).store(cur_start)
            (out_ends_ptr + out_count).store(cur_end)
            out_count += 1
            cur_start = next_start
            cur_end = next_end

    # 最後のスパンを出力
    (out_starts_ptr + out_count).store(cur_start)
    (out_ends_ptr + out_count).store(cur_end)
    out_count += 1

    return out_count
