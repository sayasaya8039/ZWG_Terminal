//! NVFP4 Quantization — NVIDIA 4-bit floating point format.
//!
//! Ollama 0.19 uses NVFP4 to maintain model accuracy while reducing memory
//! bandwidth. Unlike integer Q4_0, NVFP4 preserves the dynamic range of
//! floating-point numbers using a 1-bit sign + 2-bit exponent + 1-bit mantissa
//! encoding with per-block scale factors.
//!
//! Key advantage: better accuracy than Q4_0 for attention weights because
//! FP4 can represent both very small and moderate values, whereas INT4
//! is uniformly distributed across [-8, 7].
//!
//! Layout per block (block_size = 32 values):
//!   [scale: f16] [32 × fp4 packed into 16 bytes]
//!
//! FP4 encoding (E2M1):
//!   S EE M → value = (-1)^S × 2^(EE-1) × (1 + M×0.5)
//!   Special: 0b0000 = +0, 0b1000 = -0

/// NVFP4 block: 32 values quantized to 4-bit floats with one f16 scale.
#[derive(Debug, Clone)]
pub struct Nvfp4Block {
    /// Scale factor (stored as f32 for simplicity; would be f16 on GPU).
    pub scale: f32,
    /// 16 bytes = 32 × 4-bit values, packed 2 per byte.
    pub data: [u8; 16],
}

/// Encode a single f32 value into FP4 (E2M1) format.
/// Returns a 4-bit value (0x0..0xF).
fn f32_to_fp4(value: f32, scale: f32) -> u8 {
    if scale == 0.0 || value == 0.0 {
        return 0; // +0
    }
    let normalized = value / scale;
    let sign = if normalized < 0.0 { 1u8 } else { 0u8 };
    let abs_val = normalized.abs();

    // E2M1 representable values (positive): 0, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0
    // Find nearest representable value
    let (exp, mantissa) = if abs_val < 0.25 {
        (0u8, 0u8) // 0
    } else if abs_val < 0.75 {
        // E2M1: e=0 → 2^(-1)=0.5; m=1 → 0.5*(1+0.5)=0.75
        (0, 1)
    } else if abs_val < 1.25 {
        (1, 0) // 1.0 = 2^(1-1) × 1.0
    } else if abs_val < 1.75 {
        (1, 1) // 1.5 = 2^(1-1) × 1.5
    } else if abs_val < 2.5 {
        (2, 0) // 2.0 = 2^(2-1) × 1.0
    } else if abs_val < 3.5 {
        (2, 1) // 3.0 = 2^(2-1) × 1.5
    } else if abs_val < 5.0 {
        (3, 0) // 4.0 = 2^(3-1) × 1.0
    } else {
        (3, 1) // 6.0 = 2^(3-1) × 1.5 (max representable)
    };

    (sign << 3) | (exp << 1) | mantissa
}

/// Decode a 4-bit FP4 (E2M1) value back to f32.
fn fp4_to_f32(bits: u8, scale: f32) -> f32 {
    let sign = (bits >> 3) & 1;
    let exp = (bits >> 1) & 3;
    let mantissa = bits & 1;

    if exp == 0 && mantissa == 0 {
        return 0.0; // ±0
    }

    // value = 2^(exp-1) × (1 + mantissa × 0.5)
    let base = if exp == 0 {
        0.5 // subnormal: 2^(-1) = 0.5
    } else {
        (1u32 << (exp - 1)) as f32
    };
    let value = base * (1.0 + mantissa as f32 * 0.5);
    let signed = if sign == 1 { -value } else { value };
    signed * scale
}

/// Quantize a block of 32 f32 values to NVFP4.
pub fn quantize_block(values: &[f32]) -> Nvfp4Block {
    debug_assert!(values.len() <= 32);

    // Compute scale as max absolute value / 6.0 (max FP4 representable)
    let max_abs = values.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
    let scale = if max_abs > 0.0 { max_abs / 6.0 } else { 1.0 };

    let mut data = [0u8; 16];
    for (i, pair) in values.chunks(2).enumerate() {
        let q0 = f32_to_fp4(pair[0], scale);
        let q1 = if pair.len() > 1 {
            f32_to_fp4(pair[1], scale)
        } else {
            0
        };
        data[i] = (q0 & 0x0F) | ((q1 & 0x0F) << 4);
    }

    Nvfp4Block { scale, data }
}

/// Dequantize an NVFP4 block back to 32 f32 values.
pub fn dequantize_block(block: &Nvfp4Block, count: usize) -> Vec<f32> {
    let mut result = Vec::with_capacity(count.min(32));
    for i in 0..count.min(32) {
        let byte_idx = i / 2;
        let bits = if i % 2 == 0 {
            block.data[byte_idx] & 0x0F
        } else {
            (block.data[byte_idx] >> 4) & 0x0F
        };
        result.push(fp4_to_f32(bits, block.scale));
    }
    result
}

/// Quantize a full tensor to NVFP4 format.
pub struct Nvfp4Tensor {
    pub blocks: Vec<Nvfp4Block>,
    pub num_elements: usize,
}

impl Nvfp4Tensor {
    /// Quantize f32 data to NVFP4.
    pub fn quantize(data: &[f32]) -> Self {
        let blocks: Vec<Nvfp4Block> = data.chunks(32).map(quantize_block).collect();
        Self {
            blocks,
            num_elements: data.len(),
        }
    }

    /// Dequantize back to f32.
    pub fn dequantize(&self) -> Vec<f32> {
        let mut result = Vec::with_capacity(self.num_elements);
        let mut remaining = self.num_elements;
        for block in &self.blocks {
            let count = remaining.min(32);
            result.extend(dequantize_block(block, count));
            remaining -= count;
        }
        result
    }

    /// Memory usage in bytes.
    pub fn size_bytes(&self) -> usize {
        // 4 bytes scale + 16 bytes data per block
        self.blocks.len() * 20
    }

    /// Original f32 size in bytes.
    pub fn original_bytes(&self) -> usize {
        self.num_elements * 4
    }

    /// Compression ratio (typically ~5x for NVFP4 vs f32).
    pub fn compression_ratio(&self) -> f32 {
        self.original_bytes() as f32 / self.size_bytes().max(1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fp4_zero_roundtrip() {
        assert_eq!(fp4_to_f32(f32_to_fp4(0.0, 1.0), 1.0), 0.0);
    }

    #[test]
    fn fp4_positive_values() {
        let scale = 1.0;
        // FP4 can represent: 0, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0
        let val = fp4_to_f32(f32_to_fp4(1.0, scale), scale);
        assert!((val - 1.0).abs() < 0.01);

        let val = fp4_to_f32(f32_to_fp4(3.0, scale), scale);
        assert!((val - 3.0).abs() < 0.01);
    }

    #[test]
    fn fp4_negative_values() {
        let scale = 1.0;
        let val = fp4_to_f32(f32_to_fp4(-2.0, scale), scale);
        assert!((val - (-2.0)).abs() < 0.01);
    }

    #[test]
    fn nvfp4_tensor_compression_ratio() {
        let data: Vec<f32> = (0..128).map(|i| (i as f32 - 64.0) * 0.1).collect();
        let tensor = Nvfp4Tensor::quantize(&data);
        // NVFP4: 20 bytes per 32 values vs 128 bytes f32 → ~6.4x
        assert!(tensor.compression_ratio() > 5.0);
    }

    #[test]
    fn nvfp4_roundtrip_fidelity() {
        let data: Vec<f32> = vec![0.0, 1.0, -1.0, 2.5, -3.0, 0.5, -0.5, 4.0];
        let tensor = Nvfp4Tensor::quantize(&data);
        let recovered = tensor.dequantize();
        assert_eq!(recovered.len(), data.len());
        for (orig, rec) in data.iter().zip(recovered.iter()) {
            // FP4 has limited precision, allow ~50% relative error for small values
            let err = (orig - rec).abs();
            let tol = orig.abs() * 0.6 + 0.5; // relative + absolute tolerance
            assert!(err < tol, "NVFP4 error too large: orig={}, rec={}", orig, rec);
        }
    }
}
