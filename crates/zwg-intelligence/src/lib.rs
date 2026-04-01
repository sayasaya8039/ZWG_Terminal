#[cfg(feature = "burn_backend")]
pub mod burn_backend;

pub mod classifier;
pub mod clipboard;
pub mod context_manager;
mod device;
pub mod embeddings;
pub mod fast_io;
pub mod flash_attention;
pub mod input_accel;
mod model_manager;
pub mod multi_nic;
pub mod kv_quantizer;
pub mod nvfp4;
#[cfg(feature = "ollama")]
pub mod ollama_client;
pub mod paged_attention;
pub mod parallel_decode;
pub mod predictor;
pub mod ramdisk;
pub mod render_accel;
mod runtime;
pub mod semantic_index;
pub mod sliding_window;
pub mod speculative_decoder;
pub mod summarizer;

pub mod kv_cache;
pub use kv_cache::KvPrefixCache;

pub use classifier::{LineClassification, LineKind, classify_line, classify_lines};
pub use clipboard::SmartClipboard;
pub use context_manager::ContextManager;
pub use device::{AcceleratorDevice, DeviceKind, discover_devices};
pub use flash_attention::{FlashAttentionConfig, flash_attention_forward, multi_head_flash_attention};
pub use kv_quantizer::{CrossConversationCache, KvQuantFormat, QuantizedTensor};
pub use fast_io::{FastReader, FastWriter};
pub use model_manager::{ModelManager, ModelSpec};
pub use multi_nic::{MultiNicDispatcher, NicInfo, NicKind, discover_nics};
pub use nvfp4::Nvfp4Tensor;
#[cfg(feature = "ollama")]
pub use ollama_client::OllamaClient;
pub use paged_attention::PagedKvCache;
pub use parallel_decode::{BeamSearch, ChunkedPrefillConfig, chunked_prefill, best_of_n};
pub use ramdisk::{RamDisk, RamDiskConfig, RamDiskFs};
pub use predictor::CommandPredictor;
pub use runtime::IntelligenceRuntime;
pub use sliding_window::{ModelKeepAlive, SlidingWindowConfig, SlidingWindowContext};
pub use speculative_decoder::{NgramDraftGenerator, SpeculativeConfig, SpeculativeDecoder};
pub use summarizer::{OutputSummary, Severity, summarize};
