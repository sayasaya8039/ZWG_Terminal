#[cfg(feature = "burn_backend")]
pub mod burn_backend;

pub mod classifier;
pub mod clipboard;
pub mod context_manager;
mod device;
pub mod embeddings;
mod model_manager;
#[cfg(feature = "ollama")]
pub mod ollama_client;
pub mod predictor;
mod runtime;
pub mod semantic_index;
pub mod summarizer;

pub mod kv_cache;
pub use kv_cache::KvPrefixCache;

pub use classifier::{LineClassification, LineKind, classify_line, classify_lines};
pub use clipboard::SmartClipboard;
pub use context_manager::ContextManager;
pub use device::{AcceleratorDevice, DeviceKind, discover_devices};
pub use model_manager::{ModelManager, ModelSpec};
#[cfg(feature = "ollama")]
pub use ollama_client::OllamaClient;
pub use predictor::CommandPredictor;
pub use runtime::IntelligenceRuntime;
pub use summarizer::{OutputSummary, Severity, summarize};
