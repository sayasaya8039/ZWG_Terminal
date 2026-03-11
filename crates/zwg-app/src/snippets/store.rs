use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SNIPPET_STORE_VERSION: u32 = 1;

/// Persisted snippet entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snippet {
    pub title: String,
    pub content: String,
}

impl Default for Snippet {
    fn default() -> Self {
        Self {
            title: String::new(),
            content: String::new(),
        }
    }
}

impl Snippet {
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
        }
    }

    pub(crate) fn matches_query(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }

        self.title.to_lowercase().contains(needle) || self.content.to_lowercase().contains(needle)
    }

    fn sanitized(mut self) -> Option<Self> {
        self.title = self.title.trim().to_string();

        if self.title.is_empty() || self.content.trim().is_empty() {
            return None;
        }

        Some(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct SnippetFile {
    version: u32,
    items: Vec<Snippet>,
}

impl Default for SnippetFile {
    fn default() -> Self {
        Self {
            version: SNIPPET_STORE_VERSION,
            items: default_snippets(),
        }
    }
}

/// JSON-backed snippet storage.
#[derive(Debug, Clone)]
pub struct SnippetStore {
    path: PathBuf,
    items: Vec<Snippet>,
}

impl SnippetStore {
    pub fn load() -> Self {
        Self::load_from_path(Self::default_path())
    }

    pub fn load_from_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();

        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<SnippetFile>(&content) {
                Ok(file) => {
                    log::info!("Loaded snippets from {:?}", path);
                    Self::with_loaded_items(path, file.items)
                }
                Err(error) => {
                    log::warn!("Invalid snippet file at {:?}: {}", path, error);
                    Self::with_loaded_items(path, default_snippets())
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let store = Self::with_loaded_items(path.clone(), default_snippets());
                if let Err(save_error) = store.save() {
                    log::warn!("Failed to seed snippets at {:?}: {}", path, save_error);
                }
                store
            }
            Err(error) => {
                log::warn!("Failed to read snippets at {:?}: {}", path, error);
                Self::with_loaded_items(path, default_snippets())
            }
        }
    }

    #[allow(dead_code)]
    pub fn from_items(items: Vec<Snippet>) -> Self {
        Self::with_loaded_items(Self::default_path(), items)
    }

    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("zwg")
            .join("snippets.json")
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn items(&self) -> &[Snippet] {
        &self.items
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(&SnippetFile {
            version: SNIPPET_STORE_VERSION,
            items: self.items.clone(),
        })
        .map_err(io::Error::other)?;

        fs::write(&self.path, json)?;
        log::info!("Saved snippets to {:?}", self.path);
        Ok(())
    }

    fn with_loaded_items(path: PathBuf, items: Vec<Snippet>) -> Self {
        Self {
            path,
            items: sanitize_items(items),
        }
    }
}

fn sanitize_items(items: Vec<Snippet>) -> Vec<Snippet> {
    items.into_iter().filter_map(Snippet::sanitized).collect()
}

fn default_snippets() -> Vec<Snippet> {
    vec![
        Snippet::new("Daily Standup", "Yesterday:\n- \nToday:\n- \nBlockers:\n- "),
        Snippet::new(
            "Bug Report",
            "Summary:\nSteps to reproduce:\nExpected result:\nActual result:\nEnvironment:",
        ),
        Snippet::new(
            "Review Request",
            "Could you review this change when you have time?\nFocus areas:\n- \nKnown risks:\n- ",
        ),
        Snippet::new(
            "SSH Jump Host",
            "ssh -J user@jump.example.com user@target.example.com",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn missing_store_path_is_seeded_with_defaults() {
        let path = temp_path();
        let _ = fs::remove_file(&path);

        let store = SnippetStore::load_from_path(&path);

        assert!(!store.is_empty());
        assert!(path.exists());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_entries_are_dropped() {
        let store = SnippetStore::from_items(vec![
            Snippet::new("Valid", "content"),
            Snippet::new("   ", "content"),
            Snippet::new("Missing body", "   "),
        ]);

        assert_eq!(store.len(), 1);
        assert_eq!(store.items()[0].title, "Valid");
    }

    fn temp_path() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "zwg-snippets-{}-{}.json",
            std::process::id(),
            stamp
        ))
    }
}
