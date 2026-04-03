# paged_attention_simd.mojo — PagedAttention の SIMD 高速化
#
# KV ページ読み書きの SIMD memcpy 最適化、ゼロ初期化、ページ結合を提供。
# C ABI エクスポートで Rust FFI から呼び出し可能。

from memory import memcpy, memset_zero, UnsafePointer
from sys.info import simdwidthof


# SIMD レーン幅（f32）
alias F32_SIMD_WIDTH = simdwidthof[DType.float32]()


def _simd_zero_fill(ptr: UnsafePointer[Float32], count: Int):
    """ページ内データの SIMD 一括ゼロ初期化。

    SIMD 幅単位でゼロベクトルを書き込み、余りをスカラー処理する。
    alloc_page のリサイクル時に使用。
    """
    var zero_vec = SIMD[DType.float32, F32_SIMD_WIDTH](0.0)

    var full_chunks = count // F32_SIMD_WIDTH
    for i in range(full_chunks):
        ptr.offset(i * F32_SIMD_WIDTH).store(zero_vec)

    # 端数処理
    var remainder_start = full_chunks * F32_SIMD_WIDTH
    for i in range(remainder_start, count):
        ptr.offset(i).init_pointee_copy(0.0)


def _simd_memcpy(dst: UnsafePointer[Float32], src: UnsafePointer[Float32], count: Int):
    """KV データの SIMD memcpy 最適化。

    SIMD 幅単位でロード→ストアし、余りをスカラー処理。
    copy_from_slice / extend_from_slice の高速代替。
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


def _simd_extend(
    dst: UnsafePointer[Float32],
    dst_offset: Int,
    src: UnsafePointer[Float32],
    count: Int,
) -> Int:
    """ページ結合（read_kv）の SIMD extend。

    dst の dst_offset 位置から src を SIMD コピーし、
    コピー後の新しいオフセットを返す。
    """
    _simd_memcpy(dst.offset(dst_offset), src, count)
    return dst_offset + count


# ─── C ABI エクスポート ───


def paged_simd_zero_fill(ptr: UnsafePointer[Float32], count: Int):
    """C ABI: ページデータのゼロ初期化。

    Rust 側から alloc_page リサイクル時に呼び出す。
    """
    _simd_zero_fill(ptr, count)


def paged_simd_memcpy(
    dst: UnsafePointer[Float32], src: UnsafePointer[Float32], count: Int
):
    """C ABI: KV データの SIMD コピー。

    Rust 側の copy_from_slice 代替として使用。
    """
    _simd_memcpy(dst, src, count)


def paged_simd_extend(
    dst: UnsafePointer[Float32],
    dst_offset: Int,
    src: UnsafePointer[Float32],
    count: Int,
) -> Int:
    """C ABI: read_kv のバルクコピー。

    dst_offset から src を SIMD コピーし、新しいオフセットを返す。
    """
    return _simd_extend(dst, dst_offset, src, count)
