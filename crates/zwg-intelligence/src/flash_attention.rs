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

    let mut output = vec![0.0f32; q_len * nh * d];

    for h in 0..nh {
        // Extract per-head slices
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

        // Scatter back into interleaved output
        for i in 0..q_len {
            let out_start = i * nh * d + h * d;
            let src_start = i * d;
            output[out_start..out_start + d]
                .copy_from_slice(&head_out.values[src_start..src_start + d]);
        }
    }

    output
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
