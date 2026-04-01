use anyhow::{Context, Result, anyhow};
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;
use tokenizers::Tokenizer;

use crate::model_manager::ModelSpec;
use crate::runtime::IntelligenceRuntime;

/// Build the ModelSpec for multilingual-e5-small at runtime.
///
/// **WARNING**: The SHA-256 hash is a placeholder. Model integrity verification
/// is disabled until the real hash is set after first verified download.
// TODO: replace placeholder hash with real SHA-256 from HuggingFace
pub fn e5_small_spec() -> ModelSpec {
    ModelSpec {
        id: "multilingual-e5-small".to_string(),
        url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/onnx/model.onnx"
            .to_string(),
        sha256: "0".repeat(64),
        size_bytes: 90_000_000,
        description: "multilingual-e5-small: 384-dim embeddings, 90+ languages".to_string(),
        quantization: None,
    }
}

/// Build the ModelSpec for the optimized variant of multilingual-e5-small.
pub fn e5_small_quantized_spec() -> ModelSpec {
    ModelSpec {
        id: "multilingual-e5-small-optimized".to_string(),
        url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/onnx/model_optimized.onnx"
            .to_string(),
        sha256: "0".repeat(64),
        size_bytes: 45_000_000,
        description: "multilingual-e5-small optimized: smaller, faster inference".to_string(),
        quantization: Some("optimized".to_string()),
    }
}

/// Embedding dimension for multilingual-e5-small.
pub const EMBEDDING_DIM: usize = 384;

/// Estimated VRAM usage for the embedding model (~120 MB with DirectML overhead).
pub const ESTIMATED_VRAM: u64 = 120 * 1024 * 1024;

/// Whether the tokenizer is production-ready.
/// Set to true now that HuggingFace tokenizers crate is integrated.
const TOKENIZER_READY: bool = true;

/// Maximum sequence length for tokenization.
/// E5-small supports up to 512, but 128 is sufficient for terminal lines.
const MAX_SEQ_LEN: usize = 128;

/// Embedding engine that generates vector representations of text.
///
/// Created through `IntelligenceRuntime::load_session` to participate
/// in VRAM budget tracking and LRU eviction.
pub struct EmbeddingEngine {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
}

impl EmbeddingEngine {
    /// Load the HuggingFace tokenizer from a tokenizer.json file.
    fn load_tokenizer(tokenizer_path: &Path) -> Result<Tokenizer> {
        Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow!("failed to load tokenizer from {}: {e}", tokenizer_path.display()))
    }

    /// Create a new EmbeddingEngine using the runtime's session management.
    /// The session is tracked by the runtime for VRAM budget and eviction.
    pub fn new(runtime: &IntelligenceRuntime, model_path: &Path, tokenizer_path: &Path) -> Result<Self> {
        let tokenizer = Self::load_tokenizer(tokenizer_path)?;
        let arc_session = runtime.load_session(model_path, ESTIMATED_VRAM, "embeddings")?;
        match Arc::try_unwrap(arc_session) {
            Ok(session) => Ok(Self { session: Mutex::new(session), tokenizer }),
            Err(_arc) => {
                Self::from_file(runtime, model_path, tokenizer_path)
            }
        }
    }

    /// Fallback: create a session directly (not tracked by runtime VRAM budget).
    fn from_file(runtime: &IntelligenceRuntime, model_path: &Path, tokenizer_path: &Path) -> Result<Self> {
        let tokenizer = Self::load_tokenizer(tokenizer_path)?;
        let best = runtime.best_device()
            .context("no accelerator device available")?;
        let device_id = best.device_id;
        let kind = best.kind;

        let session = match kind {
            crate::device::DeviceKind::Npu | crate::device::DeviceKind::Gpu => {
                let ep = ort::ep::DirectML::default()
                    .with_device_id(device_id as i32)
                    .build();
                let mut builder = Session::builder()
                    .map_err(|e| anyhow!("session builder: {e}"))?
                    .with_execution_providers([ep])
                    .map_err(|e| anyhow!("DirectML EP: {e}"))?;
                builder.commit_from_file(model_path)
                    .map_err(|e| anyhow!("load model: {e}"))
            }
            crate::device::DeviceKind::Cpu => {
                let mut builder = Session::builder()
                    .map_err(|e| anyhow!("session builder: {e}"))?
                    .with_intra_threads(4)
                    .map_err(|e| anyhow!("threads: {e}"))?;
                builder.commit_from_file(model_path)
                    .map_err(|e| anyhow!("load model: {e}"))
            }
        }?;

        log::info!("EmbeddingEngine loaded from {} on {}", model_path.display(), kind);
        Ok(Self { session: Mutex::new(session), tokenizer })
    }

    /// Generate an embedding vector for a single text input.
    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embed_batch(&[text])?;
        embeddings.into_iter().next().context("empty embedding result")
    }

    /// Check if the tokenizer is production-ready.
    pub fn is_ready(&self) -> bool {
        TOKENIZER_READY
    }

    /// Tokenize a batch of texts using the HuggingFace tokenizer.
    /// Returns flattened (input_ids, attention_mask, token_type_ids) and the sequence length.
    fn tokenize(&self, texts: &[&str]) -> Result<(Vec<i64>, Vec<i64>, Vec<i64>, usize)> {
        let encodings = self.tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow!("tokenizer encode_batch failed: {e}"))?;

        // All encodings should have the same length (padding=true via truncation params),
        // but the tokenizers crate pads to the longest in the batch by default.
        // We need to ensure consistent MAX_SEQ_LEN padding/truncation.
        let seq_len = encodings.iter()
            .map(|enc| enc.get_ids().len().min(MAX_SEQ_LEN))
            .max()
            .unwrap_or(0);

        let batch_size = texts.len();
        let mut input_ids = Vec::with_capacity(batch_size * seq_len);
        let mut attention_mask = Vec::with_capacity(batch_size * seq_len);
        let mut token_type_ids = Vec::with_capacity(batch_size * seq_len);

        for enc in &encodings {
            let ids = enc.get_ids();
            let mask = enc.get_attention_mask();
            let type_ids = enc.get_type_ids();

            let actual_len = ids.len().min(seq_len);
            for i in 0..actual_len {
                input_ids.push(ids[i] as i64);
                attention_mask.push(mask[i] as i64);
                token_type_ids.push(type_ids[i] as i64);
            }
            // Pad to seq_len
            for _ in actual_len..seq_len {
                input_ids.push(0i64);
                attention_mask.push(0i64);
                token_type_ids.push(0i64);
            }
        }

        Ok((input_ids, attention_mask, token_type_ids, seq_len))
    }

    /// Chunked prefill pipeline — process large text batches in overlapping chunks.
    ///
    /// Inspired by Ollama's chunked prefill (2048 tokens/chunk).
    /// Splits `texts` into chunks of `chunk_size`, embeds each chunk via `embed_batch`,
    /// and reports progress through the callback.
    ///
    /// On partial failure, returns embeddings for all successfully processed chunks.
    pub fn embed_chunked(
        &self,
        texts: &[&str],
        chunk_size: usize,
        on_progress: impl Fn(usize, usize),
    ) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let effective_chunk = if chunk_size == 0 { 32 } else { chunk_size };
        let total = texts.len();
        let mut results = Vec::with_capacity(total);
        let mut processed = 0usize;

        for chunk in texts.chunks(effective_chunk) {
            match self.embed_batch(chunk) {
                Ok(embeddings) => {
                    results.extend(embeddings);
                    processed += chunk.len();
                    on_progress(processed, total);
                }
                Err(e) => {
                    // Return partial results on error
                    if results.is_empty() {
                        return Err(e);
                    }
                    log::warn!(
                        "embed_chunked: partial failure at {}/{}, returning {} results: {}",
                        processed, total, results.len(), e
                    );
                    return Ok(results);
                }
            }
        }

        Ok(results)
    }

    /// Parallel chunked prefill — uses rayon to process chunks on multiple CPU cores.
    ///
    /// Each chunk is independently embedded via `embed_batch`. ONNX Runtime already
    /// parallelizes within a single inference call, so inter-chunk parallelism is
    /// most beneficial when the session lock contention is low (e.g., CPU backend).
    ///
    /// Requires the `parallel` feature flag (`rayon`).
    #[cfg(feature = "parallel")]
    pub fn embed_chunked_parallel(
        &self,
        texts: &[&str],
        chunk_size: usize,
    ) -> Result<Vec<Vec<f32>>> {
        use rayon::prelude::*;

        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let effective_chunk = if chunk_size == 0 { 32 } else { chunk_size };

        // Collect chunks into owned Strings so we can send across threads
        let chunks: Vec<Vec<String>> = texts
            .chunks(effective_chunk)
            .map(|c| c.iter().map(|s| s.to_string()).collect())
            .collect();

        let chunk_results: Vec<Result<Vec<Vec<f32>>>> = chunks
            .par_iter()
            .map(|chunk| {
                let refs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
                self.embed_batch(&refs)
            })
            .collect();

        let mut results = Vec::with_capacity(texts.len());
        for chunk_result in chunk_results {
            match chunk_result {
                Ok(embeddings) => results.extend(embeddings),
                Err(e) => {
                    if results.is_empty() {
                        return Err(e);
                    }
                    log::warn!(
                        "embed_chunked_parallel: partial failure, returning {} results: {}",
                        results.len(), e
                    );
                    return Ok(results);
                }
            }
        }

        Ok(results)
    }

    /// Generate embedding vectors for a batch of texts.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let (input_ids, attention_mask, token_type_ids, seq_len) = self.tokenize(texts)?;
        let batch_size = texts.len();

        let input_ids_tensor = Tensor::from_array(
            (vec![batch_size as i64, seq_len as i64], input_ids),
        )
        .map_err(|e| anyhow!("failed to create input_ids tensor: {e}"))?;

        let attention_mask_tensor = Tensor::from_array(
            (vec![batch_size as i64, seq_len as i64], attention_mask.clone()),
        )
        .map_err(|e| anyhow!("failed to create attention_mask tensor: {e}"))?;

        let token_type_ids_tensor = Tensor::from_array(
            (vec![batch_size as i64, seq_len as i64], token_type_ids),
        )
        .map_err(|e| anyhow!("failed to create token_type_ids tensor: {e}"))?;

        let mut session = self.session.lock();
        let outputs = session
            .run(ort::inputs![
                "input_ids" => input_ids_tensor,
                "attention_mask" => attention_mask_tensor,
                "token_type_ids" => token_type_ids_tensor,
            ])
            .map_err(|e| anyhow!("ONNX inference failed: {e}"))?;

        let output_value = outputs
            .get("last_hidden_state")
            .context("no 'last_hidden_state' output from model")?;

        let (shape, data) = output_value
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("failed to extract output tensor: {e}"))?;

        if shape.len() != 3 || shape[0] as usize != batch_size || shape[2] as usize != EMBEDDING_DIM {
            return Err(anyhow!(
                "unexpected output shape: {:?}, expected [{}, ?, {}]",
                shape,
                batch_size,
                EMBEDDING_DIM
            ));
        }

        let hidden_seq_len = shape[1] as usize;
        debug_assert_eq!(hidden_seq_len, seq_len, "model output seq_len != input seq_len");

        // Mean pooling with attention mask
        let mut result = Vec::with_capacity(batch_size);
        for b in 0..batch_size {
            let mut embedding = vec![0.0f32; EMBEDDING_DIM];
            let mut total_weight = 0.0f32;

            for s in 0..hidden_seq_len {
                let mask_idx = b * seq_len + s;
                if mask_idx >= attention_mask.len() {
                    break;
                }
                let mask_val = attention_mask[mask_idx] as f32;
                if mask_val > 0.0 {
                    let offset = (b * hidden_seq_len + s) * EMBEDDING_DIM;
                    for d in 0..EMBEDDING_DIM {
                        embedding[d] += data[offset + d] * mask_val;
                    }
                    total_weight += mask_val;
                }
            }

            if total_weight > 0.0 {
                for v in &mut embedding {
                    *v /= total_weight;
                }
            }

            // L2 normalize (SIMD-accelerated)
            simd_l2_normalize(&mut embedding);

            result.push(embedding);
        }

        Ok(result)
    }
}

/// Compute cosine similarity between two L2-normalized vectors.
///
/// Uses AVX2 (8-wide) when available, SSE (4-wide) fallback, scalar tail.
/// Dynamic dispatch via `is_x86_feature_detected!`.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // Safety: AVX2 detected at runtime; a and b have equal length.
            return unsafe { cosine_similarity_avx2(a, b) };
        }
        if is_x86_feature_detected!("sse") {
            // Safety: SSE detected at runtime; a and b have equal length.
            return unsafe { cosine_similarity_sse(a, b) };
        }
    }

    cosine_similarity_scalar(a, b)
}

/// Scalar dot product fallback.
fn cosine_similarity_scalar(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// AVX2 8-wide f32 dot product.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn cosine_similarity_avx2(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let n = a.len();
    let chunks = n / 8;
    let remainder = n % 8;

    unsafe {
        // Safety: AVX2 guaranteed by target_feature + runtime detection.
        // All pointer arithmetic stays within slice bounds (offset + 8 <= n).
        let mut sum = _mm256_setzero_ps();

        for i in 0..chunks {
            let offset = i * 8;
            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));
            sum = _mm256_add_ps(sum, _mm256_mul_ps(va, vb));
        }

        // Horizontal sum of 8 f32 lanes
        let hi128 = _mm256_extractf128_ps(sum, 1);
        let lo128 = _mm256_castps256_ps128(sum);
        let sum128 = _mm_add_ps(lo128, hi128);
        let shuf = _mm_movehdup_ps(sum128);
        let sums = _mm_add_ps(sum128, shuf);
        let shuf2 = _mm_movehl_ps(sums, sums);
        let result = _mm_add_ss(sums, shuf2);
        let mut dot = _mm_cvtss_f32(result);

        // Scalar tail for remainder elements
        let tail_start = chunks * 8;
        for i in 0..remainder {
            dot += a[tail_start + i] * b[tail_start + i];
        }

        dot
    }
}

/// SSE 4-wide f32 dot product fallback.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse")]
unsafe fn cosine_similarity_sse(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let n = a.len();
    let chunks = n / 4;
    let remainder = n % 4;

    unsafe {
        // Safety: SSE guaranteed by target_feature + runtime detection.
        // All pointer arithmetic stays within slice bounds (offset + 4 <= n).
        let mut sum = _mm_setzero_ps();

        for i in 0..chunks {
            let offset = i * 4;
            let va = _mm_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm_loadu_ps(b.as_ptr().add(offset));
            sum = _mm_add_ps(sum, _mm_mul_ps(va, vb));
        }

        // Horizontal sum of 4 f32 lanes
        let shuf = _mm_movehl_ps(sum, sum);
        let sums = _mm_add_ps(sum, shuf);
        let shuf2 = _mm_shuffle_ps(sums, sums, 1);
        let result = _mm_add_ss(sums, shuf2);
        let mut dot = _mm_cvtss_f32(result);

        // Scalar tail
        let tail_start = chunks * 4;
        for i in 0..remainder {
            dot += a[tail_start + i] * b[tail_start + i];
        }

        dot
    }
}

/// L2-normalize an embedding vector in-place using SIMD.
///
/// Computes `v[i] /= ||v||_2` for all elements. No-op if norm is zero.
pub fn simd_l2_normalize(embedding: &mut [f32]) {
    let norm_sq = cosine_similarity(embedding, embedding);
    let norm = norm_sq.sqrt();
    if norm == 0.0 {
        return;
    }
    let inv_norm = 1.0 / norm;

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // Safety: AVX2 detected; inv_norm is finite (norm > 0).
            unsafe { scale_avx2(embedding, inv_norm) };
            return;
        }
    }

    for v in embedding.iter_mut() {
        *v *= inv_norm;
    }
}

/// AVX2 8-wide scalar multiplication in-place.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn scale_avx2(data: &mut [f32], scalar: f32) {
    use std::arch::x86_64::*;

    let n = data.len();
    let chunks = n / 8;

    unsafe {
        // Safety: AVX2 guaranteed by target_feature + runtime detection.
        // All pointer arithmetic stays within slice bounds.
        let vs = _mm256_set1_ps(scalar);

        for i in 0..chunks {
            let offset = i * 8;
            let v = _mm256_loadu_ps(data.as_ptr().add(offset));
            let result = _mm256_mul_ps(v, vs);
            _mm256_storeu_ps(data.as_mut_ptr().add(offset), result);
        }
    }

    let tail_start = chunks * 8;
    for v in &mut data[tail_start..] {
        *v *= scalar;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_same_vector() {
        let v = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_opposite() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn e5_spec_has_valid_url() {
        let spec = e5_small_spec();
        assert!(spec.url.starts_with("https://"));
    }

    #[test]
    fn e5_quantized_spec_has_valid_url() {
        let spec = e5_small_quantized_spec();
        assert!(spec.url.starts_with("https://"));
        assert!(spec.quantization.is_some());
    }

    #[test]
    fn tokenizer_is_ready() {
        assert!(TOKENIZER_READY);
    }

    #[test]
    fn cosine_large_vector() {
        let mut a = vec![0.0f32; EMBEDDING_DIM];
        let mut b = vec![0.0f32; EMBEDDING_DIM];
        a[0] = 1.0;
        b[0] = 1.0;
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_non_aligned_length() {
        let a = vec![1.0f32; 13];
        let b = vec![1.0f32; 13];
        assert!((cosine_similarity(&a, &b) - 13.0).abs() < 1e-4);
    }

    #[test]
    fn simd_l2_normalize_unit() {
        let mut v = vec![3.0, 4.0, 0.0];
        simd_l2_normalize(&mut v);
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
        assert!(v[2].abs() < 1e-6);
    }

    #[test]
    fn simd_l2_normalize_zero() {
        let mut v = vec![0.0; 10];
        simd_l2_normalize(&mut v);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn simd_l2_normalize_large() {
        let mut v: Vec<f32> = (0..EMBEDDING_DIM).map(|i| i as f32).collect();
        simd_l2_normalize(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
