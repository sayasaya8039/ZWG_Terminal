use anyhow::{Context, Result, anyhow};
use ort::session::Session;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::device::{AcceleratorDevice, DeviceKind, discover_devices};
use crate::model_manager::ModelManager;

#[cfg(feature = "burn_backend")]
use crate::burn_backend::BurnInferenceEngine;

/// Maximum VRAM budget for all loaded models (in bytes).
const DEFAULT_VRAM_BUDGET: u64 = 512 * 1024 * 1024; // 512 MB

/// Metadata for a loaded session.
struct SessionMeta {
    load_order: u64,
    estimated_vram: u64,
}

/// Internal state guarded by a single mutex to prevent TOCTOU races.
struct RuntimeState {
    sessions: HashMap<String, (Arc<Session>, SessionMeta)>,
    load_counter: u64,
    vram_used: u64,
}

/// Central runtime that manages ONNX sessions with DirectML/CPU fallback.
pub struct IntelligenceRuntime {
    devices: Vec<AcceleratorDevice>,
    model_manager: ModelManager,
    state: Mutex<RuntimeState>,
    vram_budget: u64,
    #[cfg(feature = "burn_backend")]
    burn_engine: Option<BurnInferenceEngine>,
}

impl IntelligenceRuntime {
    /// Initialize the intelligence runtime.
    pub fn new() -> Result<Self> {
        let devices = discover_devices()
            .context("failed to discover accelerator devices")?;

        let model_manager = ModelManager::new()
            .context("failed to initialize model manager")?;

        log::info!(
            "IntelligenceRuntime initialized: {} device(s), VRAM budget {:.0} MB",
            devices.len(),
            DEFAULT_VRAM_BUDGET as f64 / 1_048_576.0,
        );

        #[cfg(feature = "burn_backend")]
        let burn_engine = match BurnInferenceEngine::new() {
            Ok(engine) => {
                log::info!("Burn WGPU backend initialised successfully");
                Some(engine)
            }
            Err(e) => {
                log::warn!("Burn WGPU backend unavailable: {e}");
                None
            }
        };

        Ok(Self {
            devices,
            model_manager,
            state: Mutex::new(RuntimeState {
                sessions: HashMap::new(),
                load_counter: 0,
                vram_used: 0,
            }),
            vram_budget: DEFAULT_VRAM_BUDGET,
            #[cfg(feature = "burn_backend")]
            burn_engine,
        })
    }

    /// Load an ONNX model and create a session.
    /// Returns cached session if already loaded.
    pub fn load_session(
        &self,
        model_path: &Path,
        estimated_vram: u64,
        model_id: &str,
    ) -> Result<Arc<Session>> {
        // Check cache and reserve budget atomically
        {
            let mut state = self.state.lock();

            if let Some((session, _)) = state.sessions.get(model_id) {
                log::debug!("Returning cached session for '{}'", model_id);
                return Ok(Arc::clone(session));
            }

            // Evict if needed, all within the same lock
            if state.vram_used + estimated_vram > self.vram_budget {
                log::warn!(
                    "VRAM budget would exceed limit ({:.1}/{:.1} MB), evicting",
                    (state.vram_used + estimated_vram) as f64 / 1_048_576.0,
                    self.vram_budget as f64 / 1_048_576.0,
                );
                Self::evict_until_fits_inner(&mut state, self.vram_budget, estimated_vram);
            }

            // Reserve budget before releasing lock
            state.vram_used += estimated_vram;
        }

        // Create session outside lock (expensive I/O)
        let session = match self.create_session(model_path) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                // Release reserved budget on failure
                let mut state = self.state.lock();
                state.vram_used = state.vram_used.saturating_sub(estimated_vram);
                return Err(e);
            }
        };

        // Register session
        {
            let mut state = self.state.lock();
            state.load_counter += 1;
            let order = state.load_counter;
            state.sessions.insert(
                model_id.to_string(),
                (
                    Arc::clone(&session),
                    SessionMeta {
                        load_order: order,
                        estimated_vram,
                    },
                ),
            );
        }

        log::info!(
            "Loaded model '{}' ({:.1} MB VRAM)",
            model_id,
            estimated_vram as f64 / 1_048_576.0,
        );

        Ok(session)
    }

    /// Unload a specific model session.
    pub fn unload_session(&self, model_id: &str) {
        let mut state = self.state.lock();
        if let Some((_, meta)) = state.sessions.remove(model_id) {
            state.vram_used = state.vram_used.saturating_sub(meta.estimated_vram);
            log::info!("Unloaded model '{}'", model_id);
        }
    }

    /// Return the best available device, or `None` if no devices found.
    pub fn best_device(&self) -> Option<&AcceleratorDevice> {
        self.devices.first()
    }

    /// Return all discovered devices.
    pub fn devices(&self) -> &[AcceleratorDevice] {
        &self.devices
    }

    /// Return the model manager.
    pub fn model_manager(&self) -> &ModelManager {
        &self.model_manager
    }

    /// Current VRAM usage in bytes.
    pub fn vram_used(&self) -> u64 {
        self.state.lock().vram_used
    }

    /// VRAM budget in bytes.
    pub fn vram_budget(&self) -> u64 {
        self.vram_budget
    }

    /// Return the Burn WGPU inference engine, if available.
    #[cfg(feature = "burn_backend")]
    pub fn burn_engine(&self) -> Option<&BurnInferenceEngine> {
        self.burn_engine.as_ref()
    }

    /// Number of loaded sessions.
    pub fn session_count(&self) -> usize {
        self.state.lock().sessions.len()
    }

    fn create_session(&self, model_path: &Path) -> Result<Session> {
        let best = self.best_device()
            .expect("devices always contains at least CPU fallback");

        match best.kind {
            DeviceKind::Npu | DeviceKind::Gpu => {
                match self.try_directml_session(model_path, best.device_id) {
                    Ok(session) => {
                        log::info!("Session created on {} (DirectML)", best);
                        return Ok(session);
                    }
                    Err(e) => {
                        log::warn!(
                            "DirectML failed on {}: {e:#}. Falling back to CPU.",
                            best
                        );
                    }
                }
                self.try_cpu_session(model_path)
            }
            DeviceKind::Cpu => self.try_cpu_session(model_path),
        }
    }

    fn try_directml_session(&self, model_path: &Path, device_id: u32) -> Result<Session> {
        let ep = ort::ep::DirectML::default()
            .with_device_id(device_id as i32)
            .build();

        let mut builder = Session::builder()
            .map_err(|e| anyhow!("failed to create session builder: {e}"))?
            .with_execution_providers([ep])
            .map_err(|e| anyhow!("failed to set DirectML EP: {e}"))?;

        let session = builder
            .commit_from_file(model_path)
            .map_err(|e| anyhow!("failed to load model {}: {e}", model_path.display()))?;

        Ok(session)
    }

    fn try_cpu_session(&self, model_path: &Path) -> Result<Session> {
        let mut builder = Session::builder()
            .map_err(|e| anyhow!("failed to create session builder: {e}"))?
            .with_intra_threads(4)
            .map_err(|e| anyhow!("failed to set thread count: {e}"))?;

        let session = builder
            .commit_from_file(model_path)
            .map_err(|e| anyhow!("failed to load model {}: {e}", model_path.display()))?;

        log::info!("Session created on CPU (fallback)");
        Ok(session)
    }

    /// Create an optimized session with Level3 graph optimization.
    /// Used for quantized/optimized ONNX models.
    pub fn create_optimized_session(&self, model_path: &Path) -> Result<Session> {
        let best = self.best_device()
            .expect("devices always contains at least CPU fallback");

        let builder = Session::builder()
            .map_err(|e| anyhow!("failed to create session builder: {e}"))?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("failed to set optimization level: {e}"))?;

        let session = match best.kind {
            DeviceKind::Npu | DeviceKind::Gpu => {
                let ep = ort::ep::DirectML::default()
                    .with_device_id(best.device_id as i32)
                    .build();
                builder
                    .with_execution_providers([ep])
                    .map_err(|e| anyhow!("failed to set DirectML EP: {e}"))?
                    .commit_from_file(model_path)
                    .map_err(|e| anyhow!("failed to load optimized model {}: {e}", model_path.display()))?
            }
            DeviceKind::Cpu => {
                builder
                    .with_intra_threads(4)
                    .map_err(|e| anyhow!("failed to set thread count: {e}"))?
                    .commit_from_file(model_path)
                    .map_err(|e| anyhow!("failed to load optimized model {}: {e}", model_path.display()))?
            }
        };

        log::info!("Optimized session created for {} on {}", model_path.display(), best);
        Ok(session)
    }

    fn evict_until_fits_inner(state: &mut RuntimeState, budget: u64, needed: u64) {
        while state.vram_used + needed > budget && !state.sessions.is_empty() {
            let oldest_id = state
                .sessions
                .iter()
                .min_by_key(|(_, (_, meta))| meta.load_order)
                .map(|(id, _)| id.clone());

            if let Some(id) = oldest_id {
                if let Some((_, meta)) = state.sessions.remove(&id) {
                    state.vram_used = state.vram_used.saturating_sub(meta.estimated_vram);
                    log::info!(
                        "Evicted model '{}' ({:.1} MB)",
                        id,
                        meta.estimated_vram as f64 / 1_048_576.0,
                    );
                }
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_initializes() {
        let rt = IntelligenceRuntime::new();
        assert!(rt.is_ok());
        let rt = rt.unwrap();
        assert!(!rt.devices().is_empty());
        assert_eq!(rt.vram_used(), 0);
        assert_eq!(rt.session_count(), 0);
    }

    #[test]
    fn best_device_exists() {
        let rt = IntelligenceRuntime::new().unwrap();
        let best = rt.best_device();
        assert!(best.is_some());
    }
}
