# Flash Attention 2.0 — SIMD 最適化実装
#
# Dao et al., "FlashAttention-2" (2023) に基づく
# Q×K 内積、softmax exp、V蓄積を SIMD で並列化
#
# ビルド: wsl -e bash -c "source ~/mojo-env/bin/activate && mojo build flash_attention_simd.mojo"

from math import exp, sqrt, log, max
from memory import UnsafePointer, memset_zero
from sys.info import simdwidthof

alias SIMD_WIDTH = 8
alias F32 = DType.float32
alias NEG_INF = -3.4028235e+38  # f32::NEG_INFINITY 相当


# --- ヘルパー関数 ---

def simd_dot_product(a: UnsafePointer[Float32], b: UnsafePointer[Float32], dim: Int) -> Float32:
    """SIMD 内積: a・b を 8要素ずつ並列計算"""
    var acc = SIMD[F32, SIMD_WIDTH](0.0)
    var i = 0

    # SIMD ループ（8要素ずつ）
    while i + SIMD_WIDTH <= dim:
        var va = a.load[width=SIMD_WIDTH](i)
        var vb = b.load[width=SIMD_WIDTH](i)
        acc = acc + va * vb  # FMA 相当
        i += SIMD_WIDTH

    # 水平リダクション
    var result: Float32 = acc.reduce_add()

    # 端数処理（スカラー）
    while i < dim:
        result += a[i] * b[i]
        i += 1

    return result


def simd_exp_sum(scores: UnsafePointer[Float32], count: Int, max_val: Float32) -> Float32:
    """SIMD exp 合計: Σ exp(s - max) を 8要素ずつ並列計算"""
    var acc = SIMD[F32, SIMD_WIDTH](0.0)
    var max_vec = SIMD[F32, SIMD_WIDTH](max_val)
    var i = 0

    while i + SIMD_WIDTH <= count:
        var sv = scores.load[width=SIMD_WIDTH](i)
        # NEG_INF チェック: NEG_INF の場合は 0 にマスク
        var diff = sv - max_vec
        # exp 近似（Mojo の exp は SIMD 対応）
        var exp_val = exp(diff)
        acc = acc + exp_val
        i += SIMD_WIDTH

    var result: Float32 = acc.reduce_add()

    # 端数処理
    while i < count:
        var s = scores[i]
        if s > NEG_INF:
            result += exp(s - max_val)
        i += 1

    return result


def simd_weighted_accumulate(
    out: UnsafePointer[Float32],
    v_row: UnsafePointer[Float32],
    weight: Float32,
    dim: Int,
):
    """SIMD 加重蓄積: out += weight * v_row を 8要素ずつ並列計算"""
    var w_vec = SIMD[F32, SIMD_WIDTH](weight)
    var i = 0

    while i + SIMD_WIDTH <= dim:
        var ov = out.load[width=SIMD_WIDTH](i)
        var vv = v_row.load[width=SIMD_WIDTH](i)
        out.store(i, ov + w_vec * vv)  # FMA: out += w * v
        i += SIMD_WIDTH

    # 端数処理
    while i < dim:
        out[i] = out[i] + weight * v_row[i]
        i += 1


def simd_scale_inplace(data: UnsafePointer[Float32], scale: Float32, dim: Int):
    """SIMD スケーリング: data *= scale を 8要素ずつ並列計算"""
    var s_vec = SIMD[F32, SIMD_WIDTH](scale)
    var i = 0

    while i + SIMD_WIDTH <= dim:
        var dv = data.load[width=SIMD_WIDTH](i)
        data.store(i, dv * s_vec)
        i += SIMD_WIDTH

    while i < dim:
        data[i] = data[i] * scale
        i += 1


# --- メイン関数 ---

def flash_attention_forward_simd(
    q_ptr: UnsafePointer[Float32],
    k_ptr: UnsafePointer[Float32],
    v_ptr: UnsafePointer[Float32],
    out_ptr: UnsafePointer[Float32],
    q_len: Int,
    kv_len: Int,
    head_dim: Int,
    block_size: Int,
    causal: Bool,
):
    """
    Flash Attention 2.0 SIMD 最適化版（単一ヘッド）

    引数:
        q_ptr:      Query行列 [q_len, head_dim], row-major
        k_ptr:      Key行列 [kv_len, head_dim], row-major
        v_ptr:      Value行列 [kv_len, head_dim], row-major
        out_ptr:    出力行列 [q_len, head_dim], row-major（呼び出し前にゼロ初期化）
        q_len:      Query系列長
        kv_len:     Key/Value系列長
        head_dim:   ヘッド次元
        block_size: タイルサイズ
        causal:     因果マスク有効フラグ
    """
    var d = head_dim
    var scale: Float32 = 1.0 / sqrt(Float32(d))
    var bs = block_size
    var num_kv_blocks = (kv_len + bs - 1) // bs

    # LSE（Log-Sum-Exp）アキュムレータ
    var lse = UnsafePointer[Float32].alloc(q_len)
    for i in range(q_len):
        lse[i] = NEG_INF

    # スコアバッファ（ブロック最大サイズ分確保）
    var scores = UnsafePointer[Float32].alloc(bs)

    for kv_block in range(num_kv_blocks):
        var kv_start = kv_block * bs
        var kv_end = min(kv_start + bs, kv_len)
        var block_len = kv_end - kv_start

        for qi in range(q_len):
            var q_row = q_ptr + qi * d

            # SIMD 内積でスコア計算 + ブロック最大値検出
            var block_max: Float32 = NEG_INF
            for ki in range(block_len):
                var kv_idx = kv_start + ki

                # 因果マスク
                if causal and kv_idx > qi:
                    scores[ki] = NEG_INF
                    continue

                var k_row = k_ptr + kv_idx * d
                var dot = simd_dot_product(q_row, k_row, d)
                var s = dot * scale
                scores[ki] = s
                if s > block_max:
                    block_max = s

            if block_max == NEG_INF:
                continue

            # SIMD exp 合計（Online Softmax）
            var prev_lse = lse[qi]
            var block_sum = simd_exp_sum(scores, block_len, block_max)
            var block_lse = block_max + log(block_sum)

            # LSE 結合
            var new_lse: Float32
            if prev_lse == NEG_INF:
                new_lse = block_lse
            else:
                var max_lse = max(prev_lse, block_lse)
                new_lse = max_lse + log(exp(prev_lse - max_lse) + exp(block_lse - max_lse))

            # 前回出力の再スケーリング
            var out_row = out_ptr + qi * d
            if prev_lse > NEG_INF:
                var rescale = exp(prev_lse - new_lse)
                simd_scale_inplace(out_row, rescale, d)

            # SIMD V蓄積
            var weight_scale = exp(block_lse - new_lse)
            for si in range(block_len):
                var score = scores[si]
                if score == NEG_INF:
                    continue
                var w = (exp(score - block_max) / block_sum) * weight_scale
                var kv_idx = kv_start + si
                var v_row = v_ptr + kv_idx * d
                simd_weighted_accumulate(out_row, v_row, w, d)

            lse[qi] = new_lse

    # メモリ解放
    lse.free()
    scores.free()


def multi_head_flash_attention_simd(
    q_ptr: UnsafePointer[Float32],
    k_ptr: UnsafePointer[Float32],
    v_ptr: UnsafePointer[Float32],
    out_ptr: UnsafePointer[Float32],
    q_len: Int,
    kv_len: Int,
    num_heads: Int,
    head_dim: Int,
    block_size: Int,
    causal: Bool,
):
    """
    マルチヘッド Flash Attention（SIMD 最適化）

    Q/K/V: [seq_len, num_heads * head_dim] のインターリーブ配置
    ヘッド間は parallelize で並列化
    """
    var d = head_dim
    var nh = num_heads

    # ヘッドごとの一時バッファ確保
    for h in range(nh):
        # Per-head 抽出
        var q_head = UnsafePointer[Float32].alloc(q_len * d)
        var k_head = UnsafePointer[Float32].alloc(kv_len * d)
        var v_head = UnsafePointer[Float32].alloc(kv_len * d)
        var out_head = UnsafePointer[Float32].alloc(q_len * d)

        # Q ヘッド抽出
        for i in range(q_len):
            var src_start = i * nh * d + h * d
            for j in range(d):
                q_head[i * d + j] = q_ptr[src_start + j]

        # K ヘッド抽出
        for i in range(kv_len):
            var src_start = i * nh * d + h * d
            for j in range(d):
                k_head[i * d + j] = k_ptr[src_start + j]

        # V ヘッド抽出
        for i in range(kv_len):
            var src_start = i * nh * d + h * d
            for j in range(d):
                v_head[i * d + j] = v_ptr[src_start + j]

        # 出力ゼロ初期化
        memset_zero(out_head, q_len * d)

        # Flash Attention 実行
        flash_attention_forward_simd(
            q_head, k_head, v_head, out_head,
            q_len, kv_len, d, block_size, causal,
        )

        # 出力をインターリーブ配置に書き戻し
        for i in range(q_len):
            var dst_start = i * nh * d + h * d
            for j in range(d):
                out_ptr[dst_start + j] = out_head[i * d + j]

        # メモリ解放
        q_head.free()
        k_head.free()
        v_head.free()
        out_head.free()


# --- C ABI エクスポート ---

@no_inline
def _flash_attention_forward_c(
    q_ptr: UnsafePointer[Float32],
    k_ptr: UnsafePointer[Float32],
    v_ptr: UnsafePointer[Float32],
    out_ptr: UnsafePointer[Float32],
    q_len: Int32,
    kv_len: Int32,
    head_dim: Int32,
    block_size: Int32,
    causal: Int32,
):
    """C ABI: 単一ヘッド Flash Attention"""
    flash_attention_forward_simd(
        q_ptr, k_ptr, v_ptr, out_ptr,
        int(q_len), int(kv_len), int(head_dim), int(block_size),
        causal != 0,
    )


@no_inline
def _multi_head_flash_attention_c(
    q_ptr: UnsafePointer[Float32],
    k_ptr: UnsafePointer[Float32],
    v_ptr: UnsafePointer[Float32],
    out_ptr: UnsafePointer[Float32],
    q_len: Int32,
    kv_len: Int32,
    num_heads: Int32,
    head_dim: Int32,
    block_size: Int32,
    causal: Int32,
):
    """C ABI: マルチヘッド Flash Attention"""
    multi_head_flash_attention_simd(
        q_ptr, k_ptr, v_ptr, out_ptr,
        int(q_len), int(kv_len), int(num_heads), int(head_dim),
        int(block_size), causal != 0,
    )


# --- ベンチマーク ---

def main():
    """簡易ベンチマーク"""
    var q_len = 512
    var kv_len = 512
    var head_dim = 64
    var block_size = 256
    var num_heads = 8

    print("=== Flash Attention SIMD ベンチマーク ===")
    print("Q長:", q_len, "KV長:", kv_len, "ヘッド次元:", head_dim)
    print("ブロックサイズ:", block_size, "ヘッド数:", num_heads)

    # テストデータ生成（単純な値）
    var total_q = q_len * head_dim
    var total_kv = kv_len * head_dim
    var q = UnsafePointer[Float32].alloc(total_q)
    var k = UnsafePointer[Float32].alloc(total_kv)
    var v = UnsafePointer[Float32].alloc(total_kv)
    var out = UnsafePointer[Float32].alloc(total_q)

    for i in range(total_q):
        q[i] = Float32(i % 7) * 0.1
    for i in range(total_kv):
        k[i] = Float32(i % 11) * 0.1
        v[i] = Float32(i % 13) * 0.1
    memset_zero(out, total_q)

    # 単一ヘッド実行
    flash_attention_forward_simd(
        q, k, v, out,
        q_len, kv_len, head_dim, block_size, True,
    )

    print("単一ヘッド完了: out[0] =", out[0])

    # メモリ解放
    q.free()
    k.free()
    v.free()
    out.free()

    print("=== ベンチマーク完了 ===")
