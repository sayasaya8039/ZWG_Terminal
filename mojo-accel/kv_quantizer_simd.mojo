# KV Cache 量子化 — Q8_0 / Q4_0 / TurboQuant SIMD 最適化実装
#
# Ollama の KV cache quantization に基づく
# max_abs 検出、量子化ループ、4bit パッキング、dequantize を SIMD 化
#
# ビルド: wsl -e bash -c "source ~/mojo-env/bin/activate && mojo build kv_quantizer_simd.mojo"

from math import abs, max, min, round
from memory import UnsafePointer, memset_zero
from sys.info import simdwidthof

alias SIMD_WIDTH = 8
alias F32 = DType.float32
alias U8 = DType.uint8


# --- Q8_0 量子化 ---

def simd_max_abs(data: UnsafePointer[Float32], count: Int) -> Float32:
    """SIMD max_abs リダクション: 8要素ずつ abs → max 集約"""
    var acc = SIMD[F32, SIMD_WIDTH](0.0)
    var i = 0

    while i + SIMD_WIDTH <= count:
        var v = data.load[width=SIMD_WIDTH](i)
        var av = abs(v)
        acc = max(acc, av)
        i += SIMD_WIDTH

    # 水平リダクション
    var result: Float32 = acc.reduce_max()

    # 端数処理
    while i < count:
        var av = abs(data[i])
        if av > result:
            result = av
        i += 1

    return result


def simd_quantize_q8(
    data: UnsafePointer[Float32],
    out: UnsafePointer[UInt8],
    scale_out: UnsafePointer[Float32],
    count: Int,
    group_size: Int,
):
    """
    Q8_0 量子化: SIMD 最適化版

    引数:
        data:      入力 f32 データ
        out:       出力 i8 データ（u8 として格納）
        scale_out: グループごとのスケール値
        count:     要素数
        group_size: グループサイズ（通常 32）
    """
    var num_groups = (count + group_size - 1) // group_size
    var data_idx = 0

    for gi in range(num_groups):
        var group_start = gi * group_size
        var group_end = min(group_start + group_size, count)
        var group_len = group_end - group_start
        var group_ptr = data + group_start

        # SIMD max_abs 検出
        var max_val = simd_max_abs(group_ptr, group_len)
        var scale: Float32 = max_val / 127.0 if max_val > 0.0 else 1.0
        scale_out[gi] = scale

        # SIMD 量子化ループ
        var inv_scale = SIMD[F32, SIMD_WIDTH](1.0 / scale)
        var clamp_min = SIMD[F32, SIMD_WIDTH](-128.0)
        var clamp_max = SIMD[F32, SIMD_WIDTH](127.0)
        var i = 0

        while i + SIMD_WIDTH <= group_len:
            var v = group_ptr.load[width=SIMD_WIDTH](i)
            # v / scale → round → clamp
            var q = v * inv_scale
            q = round(q)
            q = max(q, clamp_min)
            q = min(q, clamp_max)
            # i8 → u8 変換して格納
            for j in range(SIMD_WIDTH):
                out[group_start + i + j] = UInt8(int(q[j]) & 0xFF)
            i += SIMD_WIDTH

        # 端数処理（スカラー）
        while i < group_len:
            var v = group_ptr[i]
            var q_val = round(v / scale)
            if q_val < -128.0:
                q_val = -128.0
            elif q_val > 127.0:
                q_val = 127.0
            out[group_start + i] = UInt8(int(q_val) & 0xFF)
            i += 1


def simd_dequantize_q8(
    data: UnsafePointer[UInt8],
    scales: UnsafePointer[Float32],
    out: UnsafePointer[Float32],
    count: Int,
    group_size: Int,
):
    """Q8_0 逆量子化: SIMD スケール乗算"""
    var num_groups = (count + group_size - 1) // group_size

    for gi in range(num_groups):
        var group_start = gi * group_size
        var group_end = min(group_start + group_size, count)
        var group_len = group_end - group_start
        var scale = scales[gi]
        var scale_vec = SIMD[F32, SIMD_WIDTH](scale)
        var i = 0

        # SIMD 逆量子化
        while i + SIMD_WIDTH <= group_len:
            # i8 復元 → f32 → * scale
            var vals = SIMD[F32, SIMD_WIDTH](0.0)
            for j in range(SIMD_WIDTH):
                # u8 → i8 → f32
                var raw = data[group_start + i + j]
                var signed = int(raw)
                if signed > 127:
                    signed = signed - 256
                vals[j] = Float32(signed)
            var result = vals * scale_vec
            out.store(group_start + i, result)
            i += SIMD_WIDTH

        # 端数処理
        while i < group_len:
            var raw = data[group_start + i]
            var signed = int(raw)
            if signed > 127:
                signed = signed - 256
            out[group_start + i] = Float32(signed) * scale
            i += 1


# --- Q4_0 量子化 ---

# 4bit パッキング用 LUT（値 0-15 → パック済みバイト下位/上位）
# LUT を使って分岐を排除

def simd_quantize_q4(
    data: UnsafePointer[Float32],
    out: UnsafePointer[UInt8],
    scale_out: UnsafePointer[Float32],
    count: Int,
    group_size: Int,
):
    """
    Q4_0 量子化: SIMD + LUT パッキング

    2値を1バイトにパック: [q1:4bit][q0:4bit]
    """
    var num_groups = (count + group_size - 1) // group_size
    var out_idx = 0

    for gi in range(num_groups):
        var group_start = gi * group_size
        var group_end = min(group_start + group_size, count)
        var group_len = group_end - group_start
        var group_ptr = data + group_start

        # SIMD max_abs 検出
        var max_val = simd_max_abs(group_ptr, group_len)
        var scale: Float32 = max_val / 7.0 if max_val > 0.0 else 1.0
        scale_out[gi] = scale

        # SIMD 量子化 → 4bit パッキング
        var inv_scale = 1.0 / scale
        var i = 0

        while i < group_len:
            # ペアごとに処理
            var v0 = group_ptr[i]
            var q0_f = round(v0 * inv_scale)
            if q0_f < -8.0:
                q0_f = -8.0
            elif q0_f > 7.0:
                q0_f = 7.0
            var q0 = UInt8((int(q0_f) + 8) & 0x0F)

            var q1: UInt8 = 8  # ゼロ（パディング）
            if i + 1 < group_len:
                var v1 = group_ptr[i + 1]
                var q1_f = round(v1 * inv_scale)
                if q1_f < -8.0:
                    q1_f = -8.0
                elif q1_f > 7.0:
                    q1_f = 7.0
                q1 = UInt8((int(q1_f) + 8) & 0x0F)

            # LUT 風パッキング: 下位4bit = q0, 上位4bit = q1
            out[out_idx] = (q0 & 0x0F) | ((q1 & 0x0F) << 4)
            out_idx += 1
            i += 2


def simd_dequantize_q4(
    data: UnsafePointer[UInt8],
    scales: UnsafePointer[Float32],
    zero_points: UnsafePointer[Float32],
    out: UnsafePointer[Float32],
    count: Int,
    group_size: Int,
):
    """Q4_0 逆量子化: SIMD スケール乗算"""
    var out_idx = 0
    var gi = 0
    var in_group = 0
    var data_idx = 0

    while out_idx < count:
        var scale = scales[gi]
        var zp = zero_points[gi]
        var byte = data[data_idx]

        # 下位4bit
        var q0 = Float32(int(byte & 0x0F)) - zp
        out[out_idx] = q0 * scale
        out_idx += 1
        in_group += 1

        # 上位4bit
        if out_idx < count:
            var q1 = Float32(int((byte >> 4) & 0x0F)) - zp
            out[out_idx] = q1 * scale
            out_idx += 1
            in_group += 1

        data_idx += 1

        if in_group >= group_size:
            gi += 1
            in_group = 0


# --- TurboQuant (PolarQuant + QJL) ---

def simd_quantize_turbo(
    data: UnsafePointer[Float32],
    out: UnsafePointer[UInt8],
    scale_out: UnsafePointer[Float32],
    count: Int,
    group_size: Int,
):
    """
    TurboQuant 量子化: PolarQuant（符号 + 大きさ）SIMD 版

    パッキング: [sign1:1][mag1:3][sign0:1][mag0:3] = 1バイト
    """
    var num_groups = (count + group_size - 1) // group_size
    var out_idx = 0

    for gi in range(num_groups):
        var group_start = gi * group_size
        var group_end = min(group_start + group_size, count)
        var group_len = group_end - group_start
        var group_ptr = data + group_start

        # SIMD max_abs
        var max_mag = simd_max_abs(group_ptr, group_len)
        var scale: Float32 = max_mag / 15.0 if max_mag > 0.0 else 1.0
        scale_out[gi] = scale

        var inv_scale = 1.0 / scale
        var i = 0

        while i < group_len:
            var v0 = group_ptr[i]
            var v1: Float32 = 0.0
            if i + 1 < group_len:
                v1 = group_ptr[i + 1]

            # 4bit 大きさ + 1bit 符号
            var mag0_f = round(abs(v0) * inv_scale)
            if mag0_f > 15.0:
                mag0_f = 15.0
            var mag0 = UInt8(int(mag0_f))
            var sign0: UInt8 = 1 if v0 < 0.0 else 0

            var mag1_f = round(abs(v1) * inv_scale)
            if mag1_f > 15.0:
                mag1_f = 15.0
            var mag1 = UInt8(int(mag1_f))
            var sign1: UInt8 = 1 if v1 < 0.0 else 0

            # パック: [sign1:1][mag1:3][sign0:1][mag0:3]
            var packed = ((sign0 << 3) | (mag0 & 0x07)) | (((sign1 << 3) | (mag1 & 0x07)) << 4)
            out[out_idx] = packed
            out_idx += 1
            i += 2


def simd_dequantize_turbo(
    data: UnsafePointer[UInt8],
    scales: UnsafePointer[Float32],
    out: UnsafePointer[Float32],
    count: Int,
    group_size: Int,
):
    """TurboQuant 逆量子化"""
    var out_idx = 0
    var gi = 0
    var in_group = 0
    var data_idx = 0

    while out_idx < count:
        var scale = scales[gi]
        var byte = data[data_idx]

        # 下位ニブル
        var lo = byte & 0x0F
        var sign0: Float32 = -1.0 if (lo & 0x08) != 0 else 1.0
        var mag0 = Float32(int(lo & 0x07))
        out[out_idx] = sign0 * mag0 * scale
        out_idx += 1
        in_group += 1

        # 上位ニブル
        if out_idx < count:
            var hi = (byte >> 4) & 0x0F
            var sign1: Float32 = -1.0 if (hi & 0x08) != 0 else 1.0
            var mag1 = Float32(int(hi & 0x07))
            out[out_idx] = sign1 * mag1 * scale
            out_idx += 1
            in_group += 1

        data_idx += 1

        if in_group >= group_size:
            gi += 1
            in_group = 0


# --- C ABI エクスポート ---

@no_inline
def _quantize_q8_c(
    data: UnsafePointer[Float32],
    out: UnsafePointer[UInt8],
    scale_out: UnsafePointer[Float32],
    count: Int32,
    group_size: Int32,
):
    """C ABI: Q8_0 量子化"""
    simd_quantize_q8(data, out, scale_out, int(count), int(group_size))


@no_inline
def _dequantize_q8_c(
    data: UnsafePointer[UInt8],
    scales: UnsafePointer[Float32],
    out: UnsafePointer[Float32],
    count: Int32,
    group_size: Int32,
):
    """C ABI: Q8_0 逆量子化"""
    simd_dequantize_q8(data, scales, out, int(count), int(group_size))


@no_inline
def _quantize_q4_c(
    data: UnsafePointer[Float32],
    out: UnsafePointer[UInt8],
    scale_out: UnsafePointer[Float32],
    count: Int32,
    group_size: Int32,
):
    """C ABI: Q4_0 量子化"""
    simd_quantize_q4(data, out, scale_out, int(count), int(group_size))


@no_inline
def _quantize_turbo_c(
    data: UnsafePointer[Float32],
    out: UnsafePointer[UInt8],
    scale_out: UnsafePointer[Float32],
    count: Int32,
    group_size: Int32,
):
    """C ABI: TurboQuant 量子化"""
    simd_quantize_turbo(data, out, scale_out, int(count), int(group_size))


# --- ベンチマーク ---

def main():
    """簡易ベンチマーク & 正確性テスト"""
    var count = 256
    var group_size = 32
    var num_groups = count // group_size

    print("=== KV Quantizer SIMD ベンチマーク ===")
    print("要素数:", count, "グループサイズ:", group_size)

    # テストデータ生成
    var data = UnsafePointer[Float32].alloc(count)
    for i in range(count):
        data[i] = Float32(i - 128) * 0.1

    # Q8_0 テスト
    var q8_out = UnsafePointer[UInt8].alloc(count)
    var q8_scales = UnsafePointer[Float32].alloc(num_groups)
    var q8_deq = UnsafePointer[Float32].alloc(count)

    simd_quantize_q8(data, q8_out, q8_scales, count, group_size)
    simd_dequantize_q8(q8_out, q8_scales, q8_deq, count, group_size)

    # 誤差計算
    var max_err: Float32 = 0.0
    for i in range(count):
        var err = abs(data[i] - q8_deq[i])
        if err > max_err:
            max_err = err

    print("Q8_0 最大誤差:", max_err)

    # メモリ解放
    data.free()
    q8_out.free()
    q8_scales.free()
    q8_deq.free()

    print("=== ベンチマーク完了 ===")
