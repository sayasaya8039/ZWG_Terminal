use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SNIPPET_STORE_VERSION: u32 = 2;
const DEFAULT_GROUP_NAME: &str = "General";

/// Persisted snippet entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snippet {
    pub title: String,
    pub content: String,
    pub group: String,
}

impl Default for Snippet {
    fn default() -> Self {
        Self {
            title: String::new(),
            content: String::new(),
            group: DEFAULT_GROUP_NAME.to_string(),
        }
    }
}

impl Snippet {
    #[allow(dead_code)]
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self::with_group(title, content, DEFAULT_GROUP_NAME)
    }

    pub fn with_group(
        title: impl Into<String>,
        content: impl Into<String>,
        group: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
            group: group.into(),
        }
    }

    pub(crate) fn matches_query(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }

        self.title.to_lowercase().contains(needle)
            || self.content.to_lowercase().contains(needle)
            || self.group.to_lowercase().contains(needle)
    }

    pub(crate) fn metric(&self) -> usize {
        self.content.chars().count().max(1)
    }

    fn sanitized(mut self) -> Option<Self> {
        self.title = self.title.trim().to_string();
        self.content = self.content.trim_end().to_string();
        self.group = sanitize_group_name(&self.group);

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
    groups: Vec<String>,
    items: Vec<Snippet>,
}

impl Default for SnippetFile {
    fn default() -> Self {
        Self {
            version: SNIPPET_STORE_VERSION,
            groups: vec![DEFAULT_GROUP_NAME.to_string()],
            items: default_snippets(),
        }
    }
}

/// JSON-backed snippet storage.
#[derive(Debug, Clone)]
pub struct SnippetStore {
    path: PathBuf,
    groups: Vec<String>,
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
                    Self::with_loaded_data(path, file.groups, file.items)
                }
                Err(error) => {
                    log::warn!("Invalid snippet file at {:?}: {}", path, error);
                    Self::with_loaded_data(
                        path,
                        vec![DEFAULT_GROUP_NAME.to_string()],
                        default_snippets(),
                    )
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let store = Self::with_loaded_data(
                    path.clone(),
                    vec![DEFAULT_GROUP_NAME.to_string()],
                    default_snippets(),
                );
                if let Err(save_error) = store.save() {
                    log::warn!("Failed to seed snippets at {:?}: {}", path, save_error);
                }
                store
            }
            Err(error) => {
                log::warn!("Failed to read snippets at {:?}: {}", path, error);
                Self::with_loaded_data(
                    path,
                    vec![DEFAULT_GROUP_NAME.to_string()],
                    default_snippets(),
                )
            }
        }
    }

    #[allow(dead_code)]
    pub fn from_items(items: Vec<Snippet>) -> Self {
        Self::with_loaded_data(Self::default_path(), Vec::new(), items)
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

    pub fn groups(&self) -> &[String] {
        &self.groups
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn add_group(&mut self, name: impl Into<String>) -> io::Result<Option<String>> {
        let candidate = sanitize_group_name(name.into());
        if self
            .groups
            .iter()
            .any(|group| group.eq_ignore_ascii_case(&candidate))
        {
            return Ok(None);
        }

        self.groups.push(candidate.clone());
        self.save()?;
        Ok(Some(candidate))
    }

    pub fn add_snippet(
        &mut self,
        title: impl Into<String>,
        content: impl Into<String>,
        group: Option<&str>,
    ) -> io::Result<Option<usize>> {
        let snippet = Snippet::with_group(
            title,
            content,
            group.unwrap_or_else(|| {
                self.groups
                    .first()
                    .map(String::as_str)
                    .unwrap_or(DEFAULT_GROUP_NAME)
            }),
        );
        let Some(snippet) = snippet.sanitized() else {
            return Ok(None);
        };

        let group_name = sanitize_group_name(&snippet.group);
        if !self
            .groups
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&group_name))
        {
            self.groups.push(group_name.clone());
        }

        self.items.push(Snippet {
            group: group_name,
            ..snippet
        });
        self.save()?;
        Ok(Some(self.items.len().saturating_sub(1)))
    }

    pub fn delete_snippet(&mut self, index: usize) -> io::Result<bool> {
        if index >= self.items.len() {
            return Ok(false);
        }

        self.items.remove(index);
        self.save()?;
        Ok(true)
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(&SnippetFile {
            version: SNIPPET_STORE_VERSION,
            groups: self.groups.clone(),
            items: self.items.clone(),
        })
        .map_err(io::Error::other)?;

        fs::write(&self.path, json)?;
        log::info!("Saved snippets to {:?}", self.path);
        Ok(())
    }

    fn with_loaded_data(path: PathBuf, groups: Vec<String>, items: Vec<Snippet>) -> Self {
        let items = sanitize_items(items);
        let groups = sanitize_groups(groups, &items);

        Self {
            path,
            groups,
            items,
        }
    }
}

fn sanitize_group_name(group: impl AsRef<str>) -> String {
    let trimmed = group.as_ref().trim();
    if trimmed.is_empty() {
        DEFAULT_GROUP_NAME.to_string()
    } else {
        trimmed.to_string()
    }
}

fn sanitize_groups(groups: Vec<String>, items: &[Snippet]) -> Vec<String> {
    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();

    for group in groups {
        let candidate = sanitize_group_name(group);
        let key = candidate.to_lowercase();
        if seen.insert(key) {
            ordered.push(candidate);
        }
    }

    for snippet in items {
        let candidate = sanitize_group_name(&snippet.group);
        let key = candidate.to_lowercase();
        if seen.insert(key) {
            ordered.push(candidate);
        }
    }

    if ordered.is_empty() {
        ordered.push(DEFAULT_GROUP_NAME.to_string());
    }

    ordered
}

fn sanitize_items(items: Vec<Snippet>) -> Vec<Snippet> {
    items.into_iter().filter_map(Snippet::sanitized).collect()
}

fn default_snippets() -> Vec<Snippet> {
    vec![
        Snippet::with_group(
            "ClaudeCode Launch",
            "claude --dangerously-skip-permissions",
            "ClaudeCode",
        ),
        Snippet::with_group("Git Status", "git status --short", "Workspace"),
        Snippet::with_group(
            "Review Template",
            "Summary:\n- \nRisk:\n- \nVerification:\n- ",
            "Workspace",
        ),
        Snippet::with_group(
            "SSH Jump Host",
            "ssh -J user@jump.example.com user@target.example.com",
            "Infra",
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

    #[test]
    fn groups_are_collected_from_snippets() {
        let store = SnippetStore::from_items(vec![
            Snippet::with_group("One", "1", "Alpha"),
            Snippet::with_group("Two", "2", "Beta"),
        ]);

        assert_eq!(store.groups(), &["Alpha".to_string(), "Beta".to_string()]);
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
