use encoding_rs::SHIFT_JIS;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

const SNIPPET_STORE_VERSION: u32 = 2;
const DEFAULT_GROUP_NAME: &str = "General";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvEncoding {
    Utf8,
    ShiftJis,
}

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
    pub const MAX_GROUPS: usize = 108;
    pub const MAX_SNIPPETS_PER_GROUP: usize = 36;

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

    pub fn csv_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("zwg")
            .join("snippets.csv")
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

    pub fn group_at(&self, index: usize) -> Option<&str> {
        self.groups.get(index).map(String::as_str)
    }

    pub fn group_index_by_name(&self, name: &str) -> Option<usize> {
        self.groups
            .iter()
            .position(|group| group.eq_ignore_ascii_case(name))
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
        if self.groups.len() >= Self::MAX_GROUPS {
            return Ok(None);
        }

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

    pub fn rename_group(&mut self, index: usize, name: impl Into<String>) -> io::Result<bool> {
        let Some(previous_name) = self.groups.get(index).cloned() else {
            return Ok(false);
        };
        let candidate = sanitize_group_name(name.into());
        if previous_name.eq_ignore_ascii_case(&candidate) {
            return Ok(false);
        }
        if self.groups.iter().enumerate().any(|(group_index, group)| {
            group_index != index && group.eq_ignore_ascii_case(&candidate)
        }) {
            return Ok(false);
        }

        self.groups[index] = candidate.clone();
        for snippet in &mut self.items {
            if snippet.group.eq_ignore_ascii_case(&previous_name) {
                snippet.group = candidate.clone();
            }
        }
        self.save()?;
        Ok(true)
    }

    pub fn move_group_up(&mut self, index: usize) -> io::Result<Option<usize>> {
        if index == 0 || index >= self.groups.len() {
            return Ok(None);
        }
        self.groups.swap(index, index - 1);
        self.save()?;
        Ok(Some(index - 1))
    }

    pub fn move_group_down(&mut self, index: usize) -> io::Result<Option<usize>> {
        if index + 1 >= self.groups.len() {
            return Ok(None);
        }
        self.groups.swap(index, index + 1);
        self.save()?;
        Ok(Some(index + 1))
    }

    pub fn delete_group(&mut self, index: usize) -> io::Result<bool> {
        if self.groups.len() <= 1 || index >= self.groups.len() {
            return Ok(false);
        }

        let removed_name = self.groups[index].clone();
        let fallback_index = if index == 0 { 1 } else { 0 };
        let fallback_group = self.groups[fallback_index].clone();

        for snippet in &mut self.items {
            if snippet.group.eq_ignore_ascii_case(&removed_name) {
                snippet.group = fallback_group.clone();
            }
        }
        self.groups.remove(index);
        self.save()?;
        Ok(true)
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
        if self
            .items
            .iter()
            .filter(|existing| existing.group.eq_ignore_ascii_case(&group_name))
            .count()
            >= Self::MAX_SNIPPETS_PER_GROUP
        {
            return Ok(None);
        }
        if !self
            .groups
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&group_name))
        {
            if self.groups.len() >= Self::MAX_GROUPS {
                return Ok(None);
            }
            self.groups.push(group_name.clone());
        }

        self.items.push(Snippet {
            group: group_name,
            ..snippet
        });
        self.save()?;
        Ok(Some(self.items.len().saturating_sub(1)))
    }

    pub fn update_snippet(
        &mut self,
        index: usize,
        title: impl Into<String>,
        content: impl Into<String>,
        group: Option<&str>,
    ) -> io::Result<bool> {
        if index >= self.items.len() {
            return Ok(false);
        }

        let desired_group = sanitize_group_name(group.unwrap_or(DEFAULT_GROUP_NAME));
        let current_group = self.items[index].group.clone();
        if !current_group.eq_ignore_ascii_case(&desired_group)
            && self
                .items
                .iter()
                .enumerate()
                .filter(|(item_index, snippet)| {
                    *item_index != index && snippet.group.eq_ignore_ascii_case(&desired_group)
                })
                .count()
                >= Self::MAX_SNIPPETS_PER_GROUP
        {
            return Ok(false);
        }

        if !self
            .groups
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&desired_group))
        {
            if self.groups.len() >= Self::MAX_GROUPS {
                return Ok(false);
            }
            self.groups.push(desired_group.clone());
        }

        let updated = Snippet::with_group(title, content, desired_group)
            .sanitized()
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "Snippet is empty"))?;
        self.items[index] = updated;
        self.save()?;
        Ok(true)
    }

    pub fn delete_snippet(&mut self, index: usize) -> io::Result<bool> {
        if index >= self.items.len() {
            return Ok(false);
        }

        self.items.remove(index);
        self.save()?;
        Ok(true)
    }

    pub fn move_snippet_up(&mut self, index: usize) -> io::Result<Option<usize>> {
        let Some(snippet) = self.items.get(index) else {
            return Ok(None);
        };
        let group_name = snippet.group.clone();
        let sibling_indices: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.group.eq_ignore_ascii_case(&group_name))
            .map(|(item_index, _)| item_index)
            .collect();
        let Some(position) = sibling_indices
            .iter()
            .position(|item_index| *item_index == index)
        else {
            return Ok(None);
        };
        if position == 0 {
            return Ok(None);
        }

        let target_index = sibling_indices[position - 1];
        self.items.swap(index, target_index);
        self.save()?;
        Ok(Some(target_index))
    }

    pub fn move_snippet_down(&mut self, index: usize) -> io::Result<Option<usize>> {
        let Some(snippet) = self.items.get(index) else {
            return Ok(None);
        };
        let group_name = snippet.group.clone();
        let sibling_indices: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.group.eq_ignore_ascii_case(&group_name))
            .map(|(item_index, _)| item_index)
            .collect();
        let Some(position) = sibling_indices
            .iter()
            .position(|item_index| *item_index == index)
        else {
            return Ok(None);
        };
        if position + 1 >= sibling_indices.len() {
            return Ok(None);
        }

        let target_index = sibling_indices[position + 1];
        self.items.swap(index, target_index);
        self.save()?;
        Ok(Some(target_index))
    }

    pub fn move_snippet_to_group(&mut self, index: usize, target_group: &str) -> io::Result<bool> {
        if index >= self.items.len() {
            return Ok(false);
        }

        let target_group = sanitize_group_name(target_group);
        if !self
            .groups
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&target_group))
        {
            return Ok(false);
        }
        if self.items[index].group.eq_ignore_ascii_case(&target_group) {
            return Ok(false);
        }
        if self
            .items
            .iter()
            .enumerate()
            .filter(|(item_index, snippet)| {
                *item_index != index && snippet.group.eq_ignore_ascii_case(&target_group)
            })
            .count()
            >= Self::MAX_SNIPPETS_PER_GROUP
        {
            return Ok(false);
        }

        self.items[index].group = target_group;
        self.save()?;
        Ok(true)
    }

    pub fn snippets_for_group(&self, group: Option<&str>) -> Vec<(usize, &Snippet)> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, snippet)| {
                group
                    .map(|group_name| snippet.group.eq_ignore_ascii_case(group_name))
                    .unwrap_or(true)
            })
            .collect()
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

    pub fn export_csv_to_path(&self, path: &Path) -> io::Result<()> {
        self.export_csv_with_encoding(path, CsvEncoding::Utf8)?;
        Ok(())
    }

    pub fn export_csv_with_encoding(&self, path: &Path, encoding: CsvEncoding) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut output = String::from("group,title,content\n");
        for snippet in &self.items {
            output.push_str(&escape_csv_field(&snippet.group));
            output.push(',');
            output.push_str(&escape_csv_field(&snippet.title));
            output.push(',');
            output.push_str(&escape_csv_field(&snippet.content));
            output.push('\n');
        }

        match encoding {
            CsvEncoding::Utf8 => fs::write(path, output),
            CsvEncoding::ShiftJis => {
                let (encoded, _encoding_used, had_errors) = SHIFT_JIS.encode(&output);
                if had_errors {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "Failed to encode CSV as Shift_JIS",
                    ));
                }
                fs::write(path, encoded.as_ref())
            }
        }
    }

    pub fn export_csv(&self) -> io::Result<PathBuf> {
        let path = Self::csv_path();
        self.export_csv_to_path(&path)?;
        Ok(path)
    }

    pub fn import_csv_from_path(&mut self, path: &Path) -> io::Result<usize> {
        self.import_csv_with_encoding(path, CsvEncoding::Utf8, true)
    }

    pub fn import_csv_with_encoding(
        &mut self,
        path: &Path,
        encoding: CsvEncoding,
        clear_before_import: bool,
    ) -> io::Result<usize> {
        let raw = fs::read(path)?;
        let content = match encoding {
            CsvEncoding::Utf8 => {
                String::from_utf8(raw).map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?
            }
            CsvEncoding::ShiftJis => {
                let (decoded, _encoding_used, had_errors) = SHIFT_JIS.decode(&raw);
                if had_errors {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "Failed to decode CSV as Shift_JIS",
                    ));
                }
                decoded.to_string()
            }
        };
        let mut items = Vec::new();

        for (row_index, row) in parse_csv_rows(&content)?.into_iter().enumerate() {
            if row.iter().all(|field| field.trim().is_empty()) {
                continue;
            }

            if row_index == 0
                && row.len() >= 3
                && row[0].eq_ignore_ascii_case("group")
                && row[1].eq_ignore_ascii_case("title")
                && row[2].eq_ignore_ascii_case("content")
            {
                continue;
            }

            let (group, title, content) = match row.as_slice() {
                [group, title, content, ..] => (group.clone(), title.clone(), content.clone()),
                [title, content] => (
                    DEFAULT_GROUP_NAME.to_string(),
                    title.clone(),
                    content.clone(),
                ),
                _ => {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        format!("Invalid CSV row at line {}", row_index + 1),
                    ));
                }
            };

            items.push(Snippet::with_group(title, content, group));
        }

        if items.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "CSV did not contain any snippets",
            ));
        }

        if clear_before_import {
            let refreshed = Self::with_loaded_data(self.path.clone(), Vec::new(), items);
            self.groups = refreshed.groups;
            self.items = refreshed.items;
        } else {
            self.items.extend(items);
            let refreshed =
                Self::with_loaded_data(self.path.clone(), self.groups.clone(), self.items.clone());
            self.groups = refreshed.groups;
            self.items = refreshed.items;
        }
        self.save()?;
        Ok(self.items.len())
    }

    pub fn import_csv(&mut self) -> io::Result<usize> {
        self.import_csv_from_path(&Self::csv_path())
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

fn escape_csv_field(field: &str) -> String {
    let escaped = field.replace('"', "\"\"");
    if escaped.contains([',', '\n', '\r', '"']) {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

fn parse_csv_rows(content: &str) -> io::Result<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = content.chars().peekable();
    let mut in_quotes = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if in_quotes {
                    if matches!(chars.peek(), Some('"')) {
                        field.push('"');
                        let _ = chars.next();
                    } else {
                        in_quotes = false;
                    }
                } else if field.is_empty() {
                    in_quotes = true;
                } else {
                    field.push(ch);
                }
            }
            ',' if !in_quotes => {
                row.push(std::mem::take(&mut field));
            }
            '\n' if !in_quotes => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            '\r' if !in_quotes => {
                if matches!(chars.peek(), Some('\n')) {
                    let _ = chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(ch),
        }
    }

    if in_quotes {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "CSV ended inside a quoted field",
        ));
    }

    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }

    Ok(rows)
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

    #[test]
    fn csv_export_import_round_trip_preserves_multiline_content() {
        let json_path = temp_path();
        let csv_path = json_path.with_extension("csv");
        let store = SnippetStore::with_loaded_data(
            json_path.clone(),
            vec![],
            vec![Snippet::with_group("One", "line1\nline2", "Alpha")],
        );

        store.export_csv_to_path(&csv_path).unwrap();

        let mut imported = SnippetStore::with_loaded_data(json_path, vec![], vec![]);
        let count = imported.import_csv_from_path(&csv_path).unwrap();

        assert_eq!(count, 1);
        assert_eq!(imported.groups(), &["Alpha".to_string()]);
        assert_eq!(imported.items()[0].content, "line1\nline2");

        let _ = fs::remove_file(csv_path);
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
