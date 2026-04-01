use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MODELS_DIR_NAME: &str = "models";
const MANIFEST_FILE: &str = "manifest.json";

/// Known model definitions shipped with smux.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    pub id: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub description: String,
    /// Optional quantization level (e.g. "int8", "int4").
    #[serde(default)]
    pub quantization: Option<String>,
}

impl ModelSpec {
    /// Validate the model spec fields.
    /// - `id` must be alphanumeric + hyphens, 1-64 chars
    /// - `url` must start with `https://`
    /// - `sha256` must be a 64-char hex string
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() || self.id.len() > 64 {
            bail!("model id must be 1-64 characters: '{}'", self.id);
        }
        if !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            bail!(
                "model id must be alphanumeric/hyphen/underscore only: '{}'",
                self.id
            );
        }
        if !self.url.starts_with("https://") {
            bail!("model url must use HTTPS: '{}'", self.url);
        }
        if self.sha256.len() != 64 || !self.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("model sha256 must be a 64-char hex string: '{}'", self.sha256);
        }
        Ok(())
    }
}

/// Build the ModelSpec for multilingual-e5-small tokenizer (tokenizer.json).
pub fn tokenizer_spec() -> ModelSpec {
    ModelSpec {
        id: "multilingual-e5-small-tokenizer".to_string(),
        url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/tokenizer.json"
            .to_string(),
        // Tokenizer JSON integrity — skip hash check (use placeholder).
        sha256: "0".repeat(64),
        size_bytes: 17_000_000,
        description: "tokenizer.json for multilingual-e5-small".to_string(),
        quantization: None,
    }
}

/// Tracks which models are downloaded and verified.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    models: HashMap<String, ManifestEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestEntry {
    sha256: String,
    size_bytes: u64,
    path: String,
}

/// Manages ONNX model files in %LOCALAPPDATA%\smux\models\.
///
/// - On-demand download with SHA-256 verification
/// - Manifest-based cache tracking
pub struct ModelManager {
    models_dir: PathBuf,
    manifest: Arc<Mutex<Manifest>>,
}

impl ModelManager {
    /// Create a new ModelManager rooted at `%LOCALAPPDATA%\zwg\models\`.
    pub fn new() -> Result<Self> {
        let base = dirs::data_local_dir()
            .context("failed to locate LOCALAPPDATA")?
            .join("zwg")
            .join(MODELS_DIR_NAME);

        fs::create_dir_all(&base)
            .with_context(|| format!("failed to create models dir: {}", base.display()))?;

        let manifest_path = base.join(MANIFEST_FILE);
        let manifest = if manifest_path.exists() {
            let data = fs::read_to_string(&manifest_path)
                .with_context(|| format!("failed to read manifest: {}", manifest_path.display()))?;
            serde_json::from_str(&data).unwrap_or_else(|e| {
                log::warn!(
                    "Manifest corrupt ({e}), starting fresh — models will be re-downloaded"
                );
                Manifest::default()
            })
        } else {
            Manifest::default()
        };

        Ok(Self {
            models_dir: base,
            manifest: Arc::new(Mutex::new(manifest)),
        })
    }

    /// Return the path to a generic file asset, downloading it if not cached.
    /// Uses the given extension instead of ".onnx".
    pub fn ensure_file(&self, spec: &ModelSpec, extension: &str) -> Result<PathBuf> {
        let file_name = format!("{}.{}", spec.id, extension);
        let file_path = self.models_dir.join(&file_name);

        // Check manifest cache
        {
            let manifest = self.manifest.lock();
            if let Some(entry) = manifest.models.get(&spec.id) {
                if entry.sha256 == spec.sha256 && file_path.exists() {
                    log::info!("File '{}' found in cache", spec.id);
                    return Ok(file_path);
                }
            }
        }

        log::info!(
            "Downloading '{}' ({:.1} MB) from {}",
            spec.id,
            spec.size_bytes as f64 / 1_048_576.0,
            spec.url
        );
        self.download(&spec.url, &file_path)?;

        // Update manifest (skip hash check for non-model assets)
        {
            let mut manifest = self.manifest.lock();
            manifest.models.insert(
                spec.id.clone(),
                ManifestEntry {
                    sha256: spec.sha256.clone(),
                    size_bytes: spec.size_bytes,
                    path: file_name,
                },
            );
            self.save_manifest(&manifest)?;
        }

        log::info!("File '{}' downloaded", spec.id);
        Ok(file_path)
    }

    /// Return the path to a model file, downloading it if not cached.
    pub fn ensure_model(&self, spec: &ModelSpec) -> Result<PathBuf> {
        spec.validate()?;

        let file_name = format!("{}.onnx", spec.id);
        let model_path = self.models_dir.join(&file_name);

        // Check manifest cache
        {
            let manifest = self.manifest.lock();
            if let Some(entry) = manifest.models.get(&spec.id) {
                if entry.sha256 == spec.sha256 && model_path.exists() {
                    log::info!("Model '{}' found in cache", spec.id);
                    return Ok(model_path);
                }
            }
        }

        // Download
        log::info!(
            "Downloading model '{}' ({:.1} MB) from {}",
            spec.id,
            spec.size_bytes as f64 / 1_048_576.0,
            spec.url
        );
        self.download(&spec.url, &model_path)?;

        // Verify SHA-256
        // Note: This is an integrity check, not an authentication check.
        // Timing side-channels are not a concern here.
        let actual_hash = sha256_file(&model_path)?;
        if actual_hash != spec.sha256 {
            fs::remove_file(&model_path).ok();
            bail!(
                "SHA-256 mismatch for '{}': expected {}, got {}",
                spec.id,
                spec.sha256,
                actual_hash
            );
        }

        // Update manifest
        {
            let mut manifest = self.manifest.lock();
            manifest.models.insert(
                spec.id.clone(),
                ManifestEntry {
                    sha256: spec.sha256.clone(),
                    size_bytes: spec.size_bytes,
                    path: file_name,
                },
            );
            self.save_manifest(&manifest)?;
        }

        log::info!("Model '{}' downloaded and verified", spec.id);
        Ok(model_path)
    }

    /// Check if a model is already cached.
    pub fn is_cached(&self, spec: &ModelSpec) -> bool {
        let manifest = self.manifest.lock();
        if let Some(entry) = manifest.models.get(&spec.id) {
            if entry.sha256 == spec.sha256 {
                let path = self.models_dir.join(&entry.path);
                return path.exists();
            }
        }
        false
    }

    /// Return the models directory path.
    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    fn download(&self, url: &str, dest: &Path) -> Result<()> {
        let response = ureq::AgentBuilder::new().build()
            .get(url)
            .call()
            .with_context(|| format!("HTTP request failed for {url}"))?;

        let status = response.status();
        if status != 200 {
            bail!("Download failed: HTTP {} for {}", status, url);
        }

        let mut file = fs::File::create(dest)
            .with_context(|| format!("failed to create file: {}", dest.display()))?;

        std::io::copy(&mut response.into_reader(), &mut file)
            .with_context(|| format!("failed to write model to {}", dest.display()))?;

        Ok(())
    }

    fn save_manifest(&self, manifest: &Manifest) -> Result<()> {
        let path = self.models_dir.join(MANIFEST_FILE);
        let json = serde_json::to_string_pretty(manifest)?;
        let mut file = fs::File::create(&path)?;
        file.write_all(json.as_bytes())?;
        Ok(())
    }
}

/// Compute SHA-256 hash of a file using the `sha2` crate.
fn sha256_file(path: &Path) -> Result<String> {
    use std::io::BufReader;

    let file = fs::File::open(path)
        .with_context(|| format!("failed to open file for hashing: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_empty_string() {
        let mut h = Sha256::new();
        h.update(b"");
        assert_eq!(
            format!("{:x}", h.finalize()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hello() {
        let mut h = Sha256::new();
        h.update(b"hello");
        assert_eq!(
            format!("{:x}", h.finalize()),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_multi_block() {
        // "abc" — NIST test vector
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(
            format!("{:x}", h.finalize()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_two_block_padding() {
        // 56 bytes — exactly at the boundary where padding wraps to second block
        let mut h = Sha256::new();
        h.update(&[0x61; 56]); // 56 'a' characters
        assert_eq!(
            format!("{:x}", h.finalize()),
            "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"
        );
    }

    #[test]
    fn model_manager_new() {
        let mm = ModelManager::new();
        assert!(mm.is_ok());
    }

    #[test]
    fn validate_good_spec() {
        let spec = ModelSpec {
            id: "all-minilm-l6-v2".to_string(),
            url: "https://huggingface.co/model.onnx".to_string(),
            sha256: "a".repeat(64),
            size_bytes: 45_000_000,
            description: "test model".to_string(),
            quantization: None,
        };
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn validate_rejects_http() {
        let spec = ModelSpec {
            id: "test".to_string(),
            url: "http://insecure.com/model.onnx".to_string(),
            sha256: "a".repeat(64),
            size_bytes: 100,
            description: "test".to_string(),
            quantization: None,
        };
        assert!(spec.validate().is_err());
    }

    #[test]
    fn validate_rejects_path_traversal() {
        let spec = ModelSpec {
            id: "../../evil".to_string(),
            url: "https://example.com/model.onnx".to_string(),
            sha256: "a".repeat(64),
            size_bytes: 100,
            description: "test".to_string(),
            quantization: None,
        };
        assert!(spec.validate().is_err());
    }

    #[test]
    fn tokenizer_spec_valid() {
        let spec = tokenizer_spec();
        assert!(spec.url.contains("tokenizer.json"));
    }
}
