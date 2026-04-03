# vt_parser_simd.mojo — VT パーサ用 SIMD 高速化モジュール
#
# ASCII 範囲 (0x20-0x7E) の一括検出、ESC シーケンス検出、
# UTF-8 マルチバイト先頭バイト検出を SIMD で高速化する。

from sys.info import simdwidthof
from memory import UnsafePointer


# ── ASCII printable 連続ランスキャン ──────────────────────────────
# data_ptr から len バイトをスキャンし、先頭から連続する
# printable ASCII (0x20-0x7E) のバイト数を返す。
# Rust 側から C ABI で呼び出される。

def scan_ascii_run(data_ptr: UnsafePointer[UInt8], length: Int) -> Int:
    """ASCII printable 文字 (0x20-0x7E) の連続ランを SIMD で高速スキャン。

    戻り値: 先頭から連続する printable ASCII バイト数。
    non-ASCII または制御文字が見つかった位置で停止する。
    """
    alias simd_width = simdwidthof[DType.uint8]()

    # SIMD 比較用の定数ベクトル
    var low = SIMD[DType.uint8, simd_width](0x20)   # スペース (printable 下限)
    var high = SIMD[DType.uint8, simd_width](0x7E)   # チルダ (printable 上限)

    var pos: Int = 0

    # SIMD 幅ずつ一括処理
    while pos + simd_width <= length:
        # メモリから SIMD レジスタへロード
        var chunk = (data_ptr + pos).load[width=simd_width]()

        # printable ASCII 範囲チェック: low <= byte <= high
        var ge_low = chunk >= low
        var le_high = chunk <= high
        var is_printable = ge_low & le_high

        # 全レーンが True なら全て printable → 次のチャンクへ
        if is_printable.reduce_and():
            pos += simd_width
        else:
            # non-printable バイトが含まれる → スカラーで正確な位置を特定
            for i in range(simd_width):
                var byte = (data_ptr + pos + i).load()
                if byte < 0x20 or byte > 0x7E:
                    return pos + i
            # 到達しないはずだが安全のため
            return pos + simd_width

    # 残りのバイトをスカラー処理
    while pos < length:
        var byte = (data_ptr + pos).load()
        if byte < 0x20 or byte > 0x7E:
            break
        pos += 1

    return pos


# ── ESC シーケンス開始位置の並列検出 ──────────────────────────────
# バッファ内の ESC (0x1B) バイトを SIMD で高速検索し、
# 最初の ESC の位置を返す。見つからなければ -1。

def find_esc_position(data_ptr: UnsafePointer[UInt8], length: Int) -> Int:
    """バッファ内の最初の ESC (0x1B) 位置を SIMD で高速検索。

    戻り値: ESC の位置インデックス。見つからなければ -1。
    """
    alias simd_width = simdwidthof[DType.uint8]()

    var esc_vec = SIMD[DType.uint8, simd_width](0x1B)
    var pos: Int = 0

    # SIMD 幅ずつ一括検索
    while pos + simd_width <= length:
        var chunk = (data_ptr + pos).load[width=simd_width]()
        var matches = chunk == esc_vec

        # いずれかのレーンが True なら ESC が存在
        if matches.reduce_or():
            for i in range(simd_width):
                if (data_ptr + pos + i).load() == 0x1B:
                    return pos + i

        pos += simd_width

    # 残りバイトのスカラー処理
    while pos < length:
        if (data_ptr + pos).load() == 0x1B:
            return pos
        pos += 1

    return -1


# ── UTF-8 マルチバイト先頭バイト検出 ──────────────────────────────
# UTF-8 の先頭バイト (0xC0-0xFF) を SIMD で検出し、
# 最初のマルチバイト先頭バイトの位置を返す。

def find_multibyte_start(data_ptr: UnsafePointer[UInt8], length: Int) -> Int:
    """UTF-8 マルチバイトシーケンスの先頭バイト (>= 0xC0) を SIMD で高速検索。

    戻り値: 最初のマルチバイト先頭バイトの位置。見つからなければ -1。
    """
    alias simd_width = simdwidthof[DType.uint8]()

    # 0xC0 以上 = UTF-8 マルチバイト先頭バイト (110xxxxx 以上)
    var threshold = SIMD[DType.uint8, simd_width](0xC0)
    var pos: Int = 0

    while pos + simd_width <= length:
        var chunk = (data_ptr + pos).load[width=simd_width]()
        var is_multibyte = chunk >= threshold

        if is_multibyte.reduce_or():
            for i in range(simd_width):
                if (data_ptr + pos + i).load() >= 0xC0:
                    return pos + i

        pos += simd_width

    # 残りバイトのスカラー処理
    while pos < length:
        if (data_ptr + pos).load() >= 0xC0:
            return pos
        pos += 1

    return -1


# ── non-ASCII バイト数カウント (統計用) ────────────────────────────

def count_non_ascii(data_ptr: UnsafePointer[UInt8], length: Int) -> Int:
    """バッファ内の non-ASCII バイト (>= 0x80) の総数を SIMD で高速カウント。

    VT パーサの統計・プロファイリング用。
    """
    alias simd_width = simdwidthof[DType.uint8]()

    var ascii_max = SIMD[DType.uint8, simd_width](0x7F)
    var count: Int = 0
    var pos: Int = 0

    while pos + simd_width <= length:
        var chunk = (data_ptr + pos).load[width=simd_width]()
        var is_non_ascii = chunk > ascii_max

        # True (1) のレーンを合計
        count += int(is_non_ascii.cast[DType.uint8]().reduce_add())
        pos += simd_width

    # 残りバイト
    while pos < length:
        if (data_ptr + pos).load() > 0x7F:
            count += 1
        pos += 1

    return count
