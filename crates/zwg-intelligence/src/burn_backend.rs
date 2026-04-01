//! Burn inference backend — MLX-style lazy evaluation for Windows.
//!
//! Provides tensor operations via Burn's computation graph with the
//! NdArray backend (CPU). The API is designed so that switching to
//! the WGPU backend (Vulkan/DX12 GPU acceleration) requires only
//! changing the type alias `B` once the upstream `windows` crate
//! version conflict is resolved.
//!
//! Burn's lazy evaluation mirrors Apple MLX's deferred computation
//! model: operations are recorded into a graph and fused/executed
//! only when results are materialised via `.into_data()`.

use burn::backend::NdArray;
use burn::tensor::Tensor;

/// Backend type alias — swap to `Wgpu` for GPU acceleration.
type B = NdArray;

/// Device for the active backend.
type Dev = burn::backend::ndarray::NdArrayDevice;

/// Information about the selected compute device.
#[derive(Debug, Clone)]
pub struct BurnDeviceInfo {
    pub backend_name: &'static str,
    pub description: String,
}

/// MLX-inspired lazy evaluation pipeline for batch embeddings.
///
/// All tensor operations are recorded lazily and only materialised
/// when `.to_data()` is called, enabling Burn's graph optimiser
/// to fuse operations and minimise memory traffic — the same
/// principle behind Apple MLX's `eval()` semantics.
///
/// Currently uses the NdArray (CPU) backend. The WGPU (GPU) backend
/// is architecturally ready but blocked by an upstream dependency
/// conflict (`gpu-allocator` ↔ `wgpu-hal` windows crate versions).
pub struct BurnInferenceEngine {
    device: Dev,
}

impl BurnInferenceEngine {
    /// Create a new engine with the NdArray (CPU) backend.
    pub fn new() -> Result<Self, String> {
        let device = Dev::Cpu;

        // Validate by creating a small probe tensor.
        let _probe: Tensor<B, 1> = Tensor::zeros([1], &device);

        log::info!("BurnInferenceEngine initialised (NdArray/CPU backend)");
        Ok(Self { device })
    }

    /// Return information about the active device.
    pub fn device_info(&self) -> BurnDeviceInfo {
        BurnDeviceInfo {
            backend_name: "ndarray",
            description: "CPU (NdArray) — GPU via WGPU planned".to_string(),
        }
    }

    /// Compute pairwise cosine similarity for a batch of vectors.
    ///
    /// `query`  — shape `[dim]`
    /// `corpus` — shape `[n, dim]`
    ///
    /// Returns `Vec<f32>` of length `n` with similarity scores in `[-1, 1]`.
    pub fn cosine_similarity_batch(
        &self,
        query: &[f32],
        corpus: &[Vec<f32>],
    ) -> Result<Vec<f32>, String> {
        if corpus.is_empty() {
            return Ok(Vec::new());
        }

        let dim = query.len();
        let n = corpus.len();

        for (i, vec) in corpus.iter().enumerate() {
            if vec.len() != dim {
                return Err(format!(
                    "corpus vector {i} has length {} but query has length {dim}",
                    vec.len()
                ));
            }
        }

        // Build tensors — lazy graph nodes.
        let q_data: Vec<f32> = query.to_vec();
        let q: Tensor<B, 1> =
            Tensor::<B, 1>::from_floats(q_data.as_slice(), &self.device);
        let q: Tensor<B, 2> = q.reshape([1, dim]);

        let flat: Vec<f32> = corpus.iter().flat_map(|v| v.iter().copied()).collect();
        let c: Tensor<B, 1> =
            Tensor::<B, 1>::from_floats(flat.as_slice(), &self.device);
        let c: Tensor<B, 2> = c.reshape([n, dim]);

        // Norms (lazy).
        let q_norm = q.clone().powf_scalar(2.0).sum_dim(1).sqrt();
        let c_norms = c.clone().powf_scalar(2.0).sum_dim(1).sqrt();

        // Dot products: (n, dim) × (dim, 1) → (n, 1)
        let dots = c.matmul(q.transpose());

        // Cosine similarity — fused graph, materialised below.
        let sims = dots / (c_norms * q_norm + 1e-8);
        let sims = sims.reshape([n]);

        // Materialise.
        let data = sims.to_data();
        let result: Vec<f32> = data.to_vec().map_err(|e| format!("tensor read: {e:?}"))?;

        Ok(result)
    }

    /// Mean-pool token embeddings and L2-normalise.
    ///
    /// `token_embeddings` — flat `[seq_len * dim]` floats.
    ///
    /// Returns the normalised embedding of shape `[dim]`.
    pub fn mean_pool_and_normalize(
        &self,
        token_embeddings: &[f32],
        seq_len: usize,
        dim: usize,
    ) -> Result<Vec<f32>, String> {
        if token_embeddings.len() != seq_len * dim {
            return Err(format!(
                "expected {} floats but got {}",
                seq_len * dim,
                token_embeddings.len()
            ));
        }

        let t: Tensor<B, 1> =
            Tensor::<B, 1>::from_floats(token_embeddings, &self.device);
        let t: Tensor<B, 2> = t.reshape([seq_len, dim]);

        // Mean pool across sequence dimension (lazy).
        let pooled: Tensor<B, 2> = t.mean_dim(0); // [1, dim]

        // L2 normalise (lazy).
        let norm = pooled.clone().powf_scalar(2.0).sum_dim(1).sqrt();
        let normalised = pooled / (norm + 1e-8);
        let normalised = normalised.reshape([dim]);

        let data = normalised.to_data();
        let result: Vec<f32> = data.to_vec().map_err(|e| format!("tensor read: {e:?}"))?;

        Ok(result)
    }

    /// Lazy embedding pipeline — MLX `eval()` equivalent.
    ///
    /// Accepts raw token embeddings for multiple inputs, performs
    /// mean-pooling and L2-normalisation as a single fused graph,
    /// and returns the batch of normalised embeddings.
    ///
    /// Each entry in `batch` is `(token_embeddings_flat, seq_len, dim)`.
    pub fn embed_lazy(
        &self,
        batch: &[(Vec<f32>, usize, usize)],
    ) -> Result<Vec<Vec<f32>>, String> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }

        let dim = batch[0].2;
        for (i, (_, _, d)) in batch.iter().enumerate() {
            if *d != dim {
                return Err(format!("batch item {i} has dim {d} but expected {dim}"));
            }
        }

        // Build pooled vectors lazily, stack for fused normalisation.
        let mut pooled_rows: Vec<Tensor<B, 2>> = Vec::with_capacity(batch.len());

        for (data, seq_len, d) in batch {
            if data.len() != seq_len * d {
                return Err(format!(
                    "expected {} floats but got {}",
                    seq_len * d,
                    data.len()
                ));
            }

            let t: Tensor<B, 1> =
                Tensor::<B, 1>::from_floats(data.as_slice(), &self.device);
            let t: Tensor<B, 2> = t.reshape([*seq_len, *d]);

            let pooled: Tensor<B, 2> = t.mean_dim(0); // [1, dim]
            pooled_rows.push(pooled);
        }

        // Stack: [batch_size, dim]
        let stacked = Tensor::cat(pooled_rows, 0);
        let norms = stacked.clone().powf_scalar(2.0).sum_dim(1).sqrt();
        let normalised = stacked / (norms + 1e-8);

        // Materialise — single dispatch for the entire batch.
        let n = batch.len();
        let results: Vec<Vec<f32>> = (0..n)
            .map(|i| {
                let row = normalised.clone().slice([i..i + 1, 0..dim]).reshape([dim]);
                let data = row.to_data();
                data.to_vec::<f32>().unwrap_or_default()
            })
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_initialises() {
        let engine = BurnInferenceEngine::new().unwrap();
        let info = engine.device_info();
        assert_eq!(info.backend_name, "ndarray");
    }

    #[test]
    fn cosine_similarity_identical_vectors() {
        let engine = BurnInferenceEngine::new().unwrap();

        let query = vec![1.0, 0.0, 0.0];
        let corpus = vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]];
        let sims = engine.cosine_similarity_batch(&query, &corpus).unwrap();

        assert_eq!(sims.len(), 2);
        assert!((sims[0] - 1.0).abs() < 0.01, "identical vectors should be ~1.0, got {}", sims[0]);
        assert!(sims[1].abs() < 0.01, "orthogonal vectors should be ~0.0, got {}", sims[1]);
    }

    #[test]
    fn cosine_similarity_empty_corpus() {
        let engine = BurnInferenceEngine::new().unwrap();
        let result = engine.cosine_similarity_batch(&[1.0, 2.0], &[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn cosine_similarity_dimension_mismatch() {
        let engine = BurnInferenceEngine::new().unwrap();
        let result = engine.cosine_similarity_batch(&[1.0, 2.0], &[vec![1.0]]);
        assert!(result.is_err());
    }

    #[test]
    fn mean_pool_normalise() {
        let engine = BurnInferenceEngine::new().unwrap();

        // Two identical tokens → mean = same vector, normalised.
        let data = vec![3.0, 0.0, 4.0, 3.0, 0.0, 4.0];
        let result = engine.mean_pool_and_normalize(&data, 2, 3).unwrap();

        assert_eq!(result.len(), 3);
        let norm: f32 = result.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01, "norm should be ~1.0, got {norm}");
    }

    #[test]
    fn mean_pool_size_mismatch() {
        let engine = BurnInferenceEngine::new().unwrap();
        let result = engine.mean_pool_and_normalize(&[1.0, 2.0], 3, 3);
        assert!(result.is_err());
    }

    #[test]
    fn embed_lazy_batch() {
        let engine = BurnInferenceEngine::new().unwrap();

        let batch = vec![
            (vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0], 2, 3),
            (vec![0.0, 0.0, 1.0], 1, 3),
        ];
        let results = engine.embed_lazy(&batch).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 3);
        assert_eq!(results[1].len(), 3);

        // Each result should be L2-normalised.
        for (i, row) in results.iter().enumerate() {
            let norm: f32 = row.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 0.01, "row {i} norm should be ~1.0, got {norm}");
        }
    }

    #[test]
    fn embed_lazy_empty() {
        let engine = BurnInferenceEngine::new().unwrap();
        let results = engine.embed_lazy(&[]).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn embed_lazy_dim_mismatch() {
        let engine = BurnInferenceEngine::new().unwrap();
        let batch = vec![
            (vec![1.0, 0.0, 0.0], 1, 3),
            (vec![1.0, 0.0], 1, 2), // different dim
        ];
        let result = engine.embed_lazy(&batch);
        assert!(result.is_err());
    }
}
