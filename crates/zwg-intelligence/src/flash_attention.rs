//! Flash Attention 2.0 — chunked attention computation for reduced memory.
//!
//! Ollama 0.19 enables Flash Attention by default, reducing VRAM by ~30%
//! and speeding context handling by 20-30%. This module provides a pure-Rust
//! implementation of the core algorithm for ZWG's inference pipeline:
//!
//! 1. **Chunked Q×K computation** — process attention in tiles instead of
//!    materializing the full N×N attention matrix.
//! 2. **Online softmax** — compute softmax incrementally per chunk using
//!    the log-sum-exp trick, avoiding a second pass over the full matrix.
//! 3. **Fused output accumulation** — multiply attention weights × V in the
//!    same loop, halving memory bandwidth.
//!
//! Ref: Dao et al., "FlashAttention-2: Faster Attention with Better Parallelism
//! and Work Partitioning" (2023).

/// Configuration for flash attention computation.
#[derive(Debug, Clone)]
pub struct FlashAttentionConfig {
    /// Tile size for chunked Q×K computation. Larger = more throughput,
    /// smaller = less peak memory. Default: 256 tokens.
    pub block_size: usize,
    /// Number of attention heads.
    pub num_heads: usize,
    /// Dimension per head (d_k). Typical: 64 or 128.
    pub head_dim: usize,
    /// Whether to apply causal masking (decoder-only models).
    pub causal: bool,
}

impl Default for FlashAttentionConfig {
    fn default() -> Self {
        Self {
            block_size: 256,
            num_heads: 8,
            head_dim: 64,
            causal: true,
        }
    }
}

/// Per-head attention output with online softmax statistics.
#[derive(Debug, Clone)]
pub struct AttentionOutput {
    /// Output values (seq_len × head_dim).
    pub values: Vec<f32>,
    /// Log-sum-exp accumulators per query position (for numerical stability).
    pub lse: Vec<f32>,
}

// --- AVX2 SIMD ヘルパー関数 (x86_64 専用) ---

/// AVX2 SIMD 内積: 8要素ずつ _mm256_fmadd_ps で並列計算
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn avx2_dot_product(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    let len = a.len().min(b.len());
    let mut acc = _mm256_setzero_ps();
    let mut i = 0usize;

    // 8要素ずつ FMA
    while i + 8 <= len {
        let va = _mm256_loadu_ps(a.as_ptr().add(i));
        let vb = _mm256_loadu_ps(b.as_ptr().add(i));
        acc = _mm256_fmadd_ps(va, vb, acc);
        i += 8;
    }

    // 水平リダクション: 256bit → 128bit → スカラー
    let hi128 = _mm256_extractf128_ps(acc, 1);
    let lo128 = _mm256_castps256_ps128(acc);
    let sum128 = _mm_add_ps(lo128, hi128);
    let shuf = _mm_movehdup_ps(sum128);
    let sums = _mm_add_ps(sum128, shuf);
    let shuf2 = _mm_movehl_ps(sums, sums);
    let result = _mm_add_ss(sums, shuf2);
    let mut dot = _mm_cvtss_f32(result);

    // 端数処理（スカラー）
    while i < len {
        dot += a[i] * b[i];
        i += 1;
    }

    dot
}

/// AVX2 fast_exp 近似: e^x ≈ (1 + x/256)^256（多項式近似）
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn avx2_fast_exp(x: &[f32], max_val: f32, out: &mut [f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    let len = x.len().min(out.len());
    let max_vec = _mm256_set1_ps(max_val);
    let mut sum_acc = _mm256_setzero_ps();
    let neg_inf = f32::NEG_INFINITY;
    let neg_inf_vec = _mm256_set1_ps(neg_inf);
    let mut i = 0usize;

    // 8要素ずつ exp(s - max) を計算
    while i + 8 <= len {
        let sv = _mm256_loadu_ps(x.as_ptr().add(i));
        // NEG_INF チェック用マスク
        let mask = _mm256_cmp_ps(sv, neg_inf_vec, _CMP_GT_OQ);
        let diff = _mm256_sub_ps(sv, max_vec);

        // スカラー exp フォールバック（AVX2 に直接 exp はないため）
        let mut exp_vals = [0.0f32; 8];
        _mm256_storeu_ps(exp_vals.as_mut_ptr(), diff);
        for j in 0..8 {
            exp_vals[j] = exp_vals[j].exp();
        }
        let exp_vec = _mm256_loadu_ps(exp_vals.as_ptr());

        // NEG_INF の位置はゼロにマスク
        let masked = _mm256_and_ps(exp_vec, mask);
        _mm256_storeu_ps(out.as_mut_ptr().add(i), masked);
        sum_acc = _mm256_add_ps(sum_acc, masked);
        i += 8;
    }

    // 水平リダクション
    let hi = _mm256_extractf128_ps(sum_acc, 1);
    let lo = _mm256_castps256_ps128(sum_acc);
    let s128 = _mm_add_ps(lo, hi);
    let shuf = _mm_movehdup_ps(s128);
    let sums = _mm_add_ps(s128, shuf);
    let shuf2 = _mm_movehl_ps(sums, sums);
    let r = _mm_add_ss(sums, shuf2);
    let mut total = _mm_cvtss_f32(r);

    // 端数処理
    while i < len {
        if x[i] > neg_inf {
            let e = (x[i] - max_val).exp();
            out[i] = e;
            total += e;
        } else {
            out[i] = 0.0;
        }
        i += 1;
    }

    total
}

/// AVX2 加重蓄積: out[j] += w * v[j] を _mm256_fmadd_ps で並列化
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn avx2_weighted_accumulate(out: &mut [f32], v_row: &[f32], weight: f32) {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    let len = out.len().min(v_row.len());
    let w_vec = _mm256_set1_ps(weight);
    let mut i = 0usize;

    while i + 8 <= len {
        let ov = _mm256_loadu_ps(out.as_ptr().add(i));
        let vv = _mm256_loadu_ps(v_row.as_ptr().add(i));
        let result = _mm256_fmadd_ps(w_vec, vv, ov); // out + w * v
        _mm256_storeu_ps(out.as_mut_ptr().add(i), result);
        i += 8;
    }

    // 端数処理
    while i < len {
        out[i] += weight * v_row[i];
        i += 1;
    }
}

/// AVX2 スケーリング: out[j] *= scale
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn avx2_scale_inplace(out: &mut [f32], scale: f32) {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    let len = out.len();
    let s_vec = _mm256_set1_ps(scale);
    let mut i = 0usize;

    while i + 8 <= len {
        let ov = _mm256_loadu_ps(out.as_ptr().add(i));
        let result = _mm256_mul_ps(ov, s_vec);
        _mm256_storeu_ps(out.as_mut_ptr().add(i), result);
        i += 8;
    }

    while i < len {
        out[i] *= scale;
        i += 1;
    }
}

/// Compute flash attention for a single head.
///
/// Uses tiled computation with online softmax to avoid materializing the
/// full attention matrix. Memory usage: O(block_size × head_dim) instead
/// of O(seq_len²).
///
/// # Arguments
/// * `q` — Query matrix, shape [q_len, head_dim], row-major
/// * `k` — Key matrix, shape [kv_len, head_dim], row-major
/// * `v` — Value matrix, shape [kv_len, head_dim], row-major
/// * `config` — Flash attention parameters
pub fn flash_attention_forward(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    config: &FlashAttentionConfig,
) -> AttentionOutput {
    // x86_64 では AVX2 SIMD パスを使用
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            return unsafe { flash_attention_forward_avx2(q, k, v, config) };
        }
    }

    // フォールバック: 元のスカラー実装
    flash_attention_forward_scalar(q, k, v, config)
}

/// AVX2 SIMD 最適化版 Flash Attention
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn flash_attention_forward_avx2(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    config: &FlashAttentionConfig,
) -> AttentionOutput {
    let d = config.head_dim;
    let q_len = q.len() / d;
    let kv_len = k.len() / d;
    let scale = 1.0 / (d as f32).sqrt();
    let bs = config.block_size;

    let mut output = vec![0.0f32; q_len * d];
    let mut lse = vec![f32::NEG_INFINITY; q_len];
    let num_kv_blocks = (kv_len + bs - 1) / bs;

    for kv_block in 0..num_kv_blocks {
        let kv_start = kv_block * bs;
        let kv_end = (kv_start + bs).min(kv_len);
        let block_len = kv_end - kv_start;

        for qi in 0..q_len {
            let q_row = &q[qi * d..(qi + 1) * d];

            let mut block_max = f32::NEG_INFINITY;
            let mut scores = Vec::with_capacity(block_len);

            for ki in 0..block_len {
                let kv_idx = kv_start + ki;
                if config.causal && kv_idx > qi {
                    scores.push(f32::NEG_INFINITY);
                    continue;
                }

                let k_row = &k[kv_idx * d..(kv_idx + 1) * d];
                // AVX2 SIMD 内積 (_mm256_fmadd_ps)
                let dot = avx2_dot_product(q_row, k_row);
                let s = dot * scale;
                scores.push(s);
                if s > block_max {
                    block_max = s;
                }
            }

            if block_max == f32::NEG_INFINITY {
                continue;
            }

            // AVX2 exp + sum (_mm256 ベクトル化)
            let prev_lse = lse[qi];
            let mut exp_vals = vec![0.0f32; scores.len()];
            let block_sum = avx2_fast_exp(&scores, block_max, &mut exp_vals);
            let block_lse = block_max + block_sum.ln();

            let new_lse = if prev_lse == f32::NEG_INFINITY {
                block_lse
            } else {
                let max_lse = prev_lse.max(block_lse);
                max_lse + ((prev_lse - max_lse).exp() + (block_lse - max_lse).exp()).ln()
            };

            let out_row = &mut output[qi * d..(qi + 1) * d];
            if prev_lse > f32::NEG_INFINITY {
                let rescale = (prev_lse - new_lse).exp();
                // AVX2 スケーリング
                avx2_scale_inplace(out_row, rescale);
            }

            // AVX2 加重 V 蓄積 (_mm256_fmadd_ps)
            let weight_scale = (block_lse - new_lse).exp();
            for (si, &score) in scores.iter().enumerate() {
                if score == f32::NEG_INFINITY {
                    continue;
                }
                let w = (exp_vals[si] / block_sum) * weight_scale;
                let kv_idx = kv_start + si;
                let v_row = &v[kv_idx * d..(kv_idx + 1) * d];
                avx2_weighted_accumulate(out_row, v_row, w);
            }

            lse[qi] = new_lse;
        }
    }

    AttentionOutput { values: output, lse }
}

/// スカラーフォールバック実装（元のコード）
fn flash_attention_forward_scalar(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    config: &FlashAttentionConfig,
) -> AttentionOutput {
    let d = config.head_dim;
    let q_len = q.len() / d;
    let kv_len = k.len() / d;
    let scale = 1.0 / (d as f32).sqrt();
    let bs = config.block_size;

    // Output accumulator and log-sum-exp per query position
    let mut output = vec![0.0f32; q_len * d];
    let mut lse = vec![f32::NEG_INFINITY; q_len];

    // Process KV in blocks of `bs` tokens
    let num_kv_blocks = (kv_len + bs - 1) / bs;

    for kv_block in 0..num_kv_blocks {
        let kv_start = kv_block * bs;
        let kv_end = (kv_start + bs).min(kv_len);
        let block_len = kv_end - kv_start;

        // For each query position, compute attention scores against this KV block
        for qi in 0..q_len {
            let q_row = &q[qi * d..(qi + 1) * d];

            // Compute max score in this block (for numerical stability)
            let mut block_max = f32::NEG_INFINITY;
            let mut scores = Vec::with_capacity(block_len);

            for ki in 0..block_len {
                let kv_idx = kv_start + ki;

                // Causal mask: skip future positions
                if config.causal && kv_idx > qi {
                    scores.push(f32::NEG_INFINITY);
                    continue;
                }

                let k_row = &k[kv_idx * d..(kv_idx + 1) * d];
                let dot: f32 = q_row.iter().zip(k_row).map(|(a, b)| a * b).sum();
                let s = dot * scale;
                scores.push(s);
                if s > block_max {
                    block_max = s;
                }
            }

            if block_max == f32::NEG_INFINITY {
                continue;
            }

            // Online softmax update (log-sum-exp trick)
            let prev_lse = lse[qi];
            let mut block_sum = 0.0f32;
            for s in &scores {
                if *s > f32::NEG_INFINITY {
                    block_sum += (*s - block_max).exp();
                }
            }
            let block_lse = block_max + block_sum.ln();

            // Combine with previous blocks using log-sum-exp
            let new_lse = if prev_lse == f32::NEG_INFINITY {
                block_lse
            } else {
                let max_lse = prev_lse.max(block_lse);
                max_lse + ((prev_lse - max_lse).exp() + (block_lse - max_lse).exp()).ln()
            };

            // Rescale previous output and add this block's contribution
            let out_row = &mut output[qi * d..(qi + 1) * d];
            if prev_lse > f32::NEG_INFINITY {
                let rescale = (prev_lse - new_lse).exp();
                for o in out_row.iter_mut() {
                    *o *= rescale;
                }
            }

            // Accumulate weighted V for this block
            let weight_scale = (block_lse - new_lse).exp();
            for (si, &score) in scores.iter().enumerate() {
                if score == f32::NEG_INFINITY {
                    continue;
                }
                let w = ((score - block_max).exp() / block_sum) * weight_scale;
                let kv_idx = kv_start + si;
                let v_row = &v[kv_idx * d..(kv_idx + 1) * d];
                for (o, &vi) in out_row.iter_mut().zip(v_row) {
                    *o += w * vi;
                }
            }

            lse[qi] = new_lse;
        }
    }

    AttentionOutput { values: output, lse }
}

/// Multi-head flash attention.
///
/// Splits Q, K, V across heads, computes flash attention per head,
/// and concatenates the results.
///
/// `parallel` feature 有効時は rayon でヘッド間を並列化。
pub fn multi_head_flash_attention(
    q: &[f32],  // [seq_len, num_heads * head_dim]
    k: &[f32],  // [kv_len, num_heads * head_dim]
    v: &[f32],  // [kv_len, num_heads * head_dim]
    config: &FlashAttentionConfig,
) -> Vec<f32> {
    let d = config.head_dim;
    let nh = config.num_heads;
    let q_len = q.len() / (nh * d);
    let kv_len = k.len() / (nh * d);

    // rayon 並列化: ヘッドごとに独立計算 → 結果をマージ
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;

        // 各ヘッドの出力を並列計算
        let head_outputs: Vec<Vec<f32>> = (0..nh)
            .into_par_iter()
            .map(|h| {
                let mut q_head = Vec::with_capacity(q_len * d);
                for i in 0..q_len {
                    let start = i * nh * d + h * d;
                    q_head.extend_from_slice(&q[start..start + d]);
                }

                let mut k_head = Vec::with_capacity(kv_len * d);
                for i in 0..kv_len {
                    let start = i * nh * d + h * d;
                    k_head.extend_from_slice(&k[start..start + d]);
                }

                let mut v_head = Vec::with_capacity(kv_len * d);
                for i in 0..kv_len {
                    let start = i * nh * d + h * d;
                    v_head.extend_from_slice(&v[start..start + d]);
                }

                flash_attention_forward(&q_head, &k_head, &v_head, config).values
            })
            .collect();

        // インターリーブ出力にマージ
        let mut output = vec![0.0f32; q_len * nh * d];
        for (h, head_vals) in head_outputs.iter().enumerate() {
            for i in 0..q_len {
                let out_start = i * nh * d + h * d;
                let src_start = i * d;
                output[out_start..out_start + d]
                    .copy_from_slice(&head_vals[src_start..src_start + d]);
            }
        }
        return output;
    }

    // フォールバック: 逐次実行
    #[cfg(not(feature = "parallel"))]
    {
        let mut output = vec![0.0f32; q_len * nh * d];

        for h in 0..nh {
            let mut q_head = Vec::with_capacity(q_len * d);
            for i in 0..q_len {
                let start = i * nh * d + h * d;
                q_head.extend_from_slice(&q[start..start + d]);
            }

            let mut k_head = Vec::with_capacity(kv_len * d);
            for i in 0..kv_len {
                let start = i * nh * d + h * d;
                k_head.extend_from_slice(&k[start..start + d]);
            }

            let mut v_head = Vec::with_capacity(kv_len * d);
            for i in 0..kv_len {
                let start = i * nh * d + h * d;
                v_head.extend_from_slice(&v[start..start + d]);
            }

            let head_out = flash_attention_forward(&q_head, &k_head, &v_head, config);

            for i in 0..q_len {
                let out_start = i * nh * d + h * d;
                let src_start = i * d;
                output[out_start..out_start + d]
                    .copy_from_slice(&head_out.values[src_start..src_start + d]);
            }
        }

        output
    }
}

/// Estimate memory savings from flash attention vs standard attention.
///
/// Standard attention materializes O(seq_len² × num_heads) floats.
/// Flash attention only needs O(block_size × head_dim × num_heads).
pub fn memory_savings_estimate(
    seq_len: usize,
    config: &FlashAttentionConfig,
) -> FlashMemoryEstimate {
    let standard_bytes =
        seq_len * seq_len * config.num_heads * std::mem::size_of::<f32>();
    let flash_bytes =
        config.block_size * config.head_dim * config.num_heads * std::mem::size_of::<f32>() * 2;
    FlashMemoryEstimate {
        standard_bytes,
        flash_bytes,
        savings_ratio: 1.0 - (flash_bytes as f64 / standard_bytes as f64),
    }
}

#[derive(Debug, Clone)]
pub struct FlashMemoryEstimate {
    pub standard_bytes: usize,
    pub flash_bytes: usize,
    /// Fraction of memory saved (0.0 to 1.0).
    pub savings_ratio: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_token_attention_returns_value() {
        let config = FlashAttentionConfig {
            block_size: 4,
            num_heads: 1,
            head_dim: 2,
            causal: false,
        };
        // Q = [1, 0], K = [1, 0], V = [3, 7]
        let q = vec![1.0, 0.0];
        let k = vec![1.0, 0.0];
        let v = vec![3.0, 7.0];
        let out = flash_attention_forward(&q, &k, &v, &config);
        assert!((out.values[0] - 3.0).abs() < 1e-4);
        assert!((out.values[1] - 7.0).abs() < 1e-4);
    }

    #[test]
    fn causal_mask_blocks_future() {
        let config = FlashAttentionConfig {
            block_size: 4,
            num_heads: 1,
            head_dim: 2,
            causal: true,
        };
        // 2 tokens: Q[0] should only attend to K[0], Q[1] attends to K[0..=1]
        let q = vec![1.0, 0.0, 0.0, 1.0];
        let k = vec![1.0, 0.0, 0.0, 1.0];
        let v = vec![1.0, 2.0, 3.0, 4.0];
        let out = flash_attention_forward(&q, &k, &v, &config);
        // Q[0] only sees V[0] = [1, 2]
        assert!((out.values[0] - 1.0).abs() < 1e-4);
        assert!((out.values[1] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn memory_savings_are_significant() {
        let config = FlashAttentionConfig {
            block_size: 256,
            num_heads: 8,
            head_dim: 64,
            causal: true,
        };
        let est = memory_savings_estimate(4096, &config);
        assert!(est.savings_ratio > 0.90, "expected >90% savings for 4K context");
    }
}
