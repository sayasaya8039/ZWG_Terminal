# async_io_simd.mojo — SPSC リングバッファの SIMD 高速化
#
# リングバッファの折り返し memcpy と drain を SIMD で並列処理。
# C ABI エクスポートで Rust FFI から呼び出し可能。

from memory import UnsafePointer
from sys.info import simdwidthof


# SIMD レーン幅
alias U8_SIMD_WIDTH = simdwidthof[DType.uint8]()
alias F32_SIMD_WIDTH = simdwidthof[DType.float32]()


def _simd_ring_copy(
    dst: UnsafePointer[UInt8],
    src: UnsafePointer[UInt8],
    count: Int,
):
    """SPSC リングバッファの折り返し memcpy を SIMD 化。

    リングバッファが末尾で折り返す場合、2回の memcpy が必要になるが、
    各セグメントを SIMD 幅で高速コピーする。
    """
    var full_chunks = count // U8_SIMD_WIDTH
    for i in range(full_chunks):
        var offset = i * U8_SIMD_WIDTH
        var vec = src.offset(offset).load[width=U8_SIMD_WIDTH]()
        dst.offset(offset).store(vec)

    # 端数処理
    var remainder_start = full_chunks * U8_SIMD_WIDTH
    for i in range(remainder_start, count):
        dst.offset(i).init_pointee_copy(src.offset(i).take_pointee())


def _simd_drain_f32(
    dst: UnsafePointer[Float32],
    src: UnsafePointer[Float32],
    count: Int,
):
    """バッファ drain の並列処理（f32 用）。

    リングバッファから読み出したデータを出力バッファに
    SIMD 幅単位でバルク転送する。
    """
    var full_chunks = count // F32_SIMD_WIDTH
    for i in range(full_chunks):
        var offset = i * F32_SIMD_WIDTH
        var vec = src.offset(offset).load[width=F32_SIMD_WIDTH]()
        dst.offset(offset).store(vec)

    # 端数処理
    var remainder_start = full_chunks * F32_SIMD_WIDTH
    for i in range(remainder_start, count):
        dst.offset(i).init_pointee_copy(src.offset(i).take_pointee())


def _simd_ring_wrap_copy(
    dst: UnsafePointer[UInt8],
    src: UnsafePointer[UInt8],
    src_capacity: Int,
    src_start: Int,
    count: Int,
):
    """リングバッファの折り返しを考慮した SIMD コピー。

    src_start + count が src_capacity を超える場合、
    2つのセグメントに分割して SIMD コピーする。
    """
    var first_len = src_capacity - src_start
    if count <= first_len:
        # 折り返しなし — 一括コピー
        _simd_ring_copy(dst, src.offset(src_start), count)
    else:
        # 折り返しあり — 2セグメント
        _simd_ring_copy(dst, src.offset(src_start), first_len)
        _simd_ring_copy(dst.offset(first_len), src, count - first_len)


# ─── C ABI エクスポート ───


def async_simd_ring_copy(
    dst: UnsafePointer[UInt8],
    src: UnsafePointer[UInt8],
    count: Int,
):
    """C ABI: リングバッファセグメントの SIMD コピー。"""
    _simd_ring_copy(dst, src, count)


def async_simd_drain_f32(
    dst: UnsafePointer[Float32],
    src: UnsafePointer[Float32],
    count: Int,
):
    """C ABI: f32 バッファの SIMD ドレイン。"""
    _simd_drain_f32(dst, src, count)


def async_simd_ring_wrap_copy(
    dst: UnsafePointer[UInt8],
    src: UnsafePointer[UInt8],
    src_capacity: Int,
    src_start: Int,
    count: Int,
):
    """C ABI: 折り返し対応リングバッファ SIMD コピー。"""
    _simd_ring_wrap_copy(dst, src, src_capacity, src_start, count)
