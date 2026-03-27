use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::clipboard_monitor::ClipboardCapture;

const HISTORY_FILE_NAME: &str = "clipboard-history.json";
const TEMPLATE_FILE_NAME: &str = "snippet-templates.json";
const HISTORY_STORE_VERSION: u32 = 1;
const TEMPLATE_STORE_VERSION: u32 = 1;
const HISTORY_LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnippetSection {
    History,
    Template,
}

impl SnippetSection {
    pub fn title(self) -> &'static str {
        match self {
            Self::History => "履歴",
            Self::Template => "定型文",
        }
    }

    pub fn empty_label(self) -> &'static str {
        match self {
            Self::History => "履歴がありません",
            Self::Template => "定型文がありません",
        }
    }

    fn all() -> [Self; 2] {
        [Self::History, Self::Template]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetRecord {
    pub id: String,
    pub section: SnippetSection,
    pub kind_label: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub note: Option<String>,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub source: String,
    pub created_label: String,
    pub captured_at_epoch_secs: Option<u64>,
}

impl SnippetRecord {
    pub fn relative_created_label(&self) -> Option<String> {
        self.captured_at_epoch_secs.map(format_relative_epoch_secs)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetPaletteModel {
    snippets: Vec<SnippetRecord>,
    active_section: SnippetSection,
    selected_snippet_id: Option<String>,
    search_query: String,
    pinned_only: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedHistory {
    version: u32,
    records: Vec<SnippetRecord>,
}

impl SnippetPaletteModel {
    pub fn new() -> Self {
        Self::from_records(load_history_records(), load_template_records())
    }

    #[cfg(test)]
    pub fn with_test_data() -> Self {
        Self::from_records(sample_history_records(), default_template_records())
    }

    fn from_records(mut history: Vec<SnippetRecord>, templates: Vec<SnippetRecord>) -> Self {
        history.retain(|record| record.section == SnippetSection::History);
        let mut snippets = history;
        snippets.extend(
            templates
                .into_iter()
                .filter(|record| record.section == SnippetSection::Template),
        );

        let mut model = Self {
            snippets,
            active_section: SnippetSection::History,
            selected_snippet_id: None,
            search_query: String::new(),
            pinned_only: false,
        };
        model.sync_selection();
        model
    }

    #[cfg(test)]
    pub fn snippets(&self) -> &[SnippetRecord] {
        &self.snippets
    }

    pub fn active_section(&self) -> SnippetSection {
        self.active_section
    }

    pub fn has_history_items(&self) -> bool {
        self.snippets
            .iter()
            .any(|snippet| snippet.section == SnippetSection::History)
    }

    pub fn section_total_count(&self, section: SnippetSection) -> usize {
        self.snippets
            .iter()
            .filter(|snippet| snippet.section == section)
            .count()
    }

    pub fn section_pinned_count(&self, section: SnippetSection) -> usize {
        self.snippets
            .iter()
            .filter(|snippet| snippet.section == section && snippet.pinned)
            .count()
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn pinned_only(&self) -> bool {
        self.pinned_only
    }

    pub fn total_count(&self) -> usize {
        self.section_total_count(self.active_section)
    }

    pub fn pinned_count(&self) -> usize {
        self.section_pinned_count(self.active_section)
    }

    #[cfg(test)]
    pub fn visible_count(&self) -> usize {
        self.visible_snippets().len()
    }

    pub fn visible_snippets(&self) -> Vec<&SnippetRecord> {
        filter_snippets(&self.snippets, &self.search_query)
            .into_iter()
            .filter(|snippet| snippet.section == self.active_section)
            .filter(|snippet| !self.pinned_only || snippet.pinned)
            .collect()
    }

    pub fn selected_snippet(&self) -> Option<&SnippetRecord> {
        let visible = self.visible_snippets();
        self.selected_snippet_id
            .as_ref()
            .and_then(|id| visible.iter().find(|snippet| snippet.id == *id).copied())
            .or_else(|| visible.first().copied())
    }

    pub fn select(&mut self, snippet_id: &str) -> bool {
        if self
            .visible_snippet_ids()
            .iter()
            .any(|candidate| candidate == snippet_id)
        {
            self.selected_snippet_id = Some(snippet_id.to_string());
            return true;
        }

        false
    }

    pub fn select_section(&mut self, section: SnippetSection) -> bool {
        if self.active_section == section {
            return false;
        }

        self.active_section = section;
        self.sync_selection();
        true
    }

    pub fn cycle_sections(&mut self, step: isize) -> bool {
        let sections = SnippetSection::all();
        let current_index = sections
            .iter()
            .position(|section| *section == self.active_section)
            .unwrap_or(0);
        let next_index =
            (current_index as isize + step).rem_euclid(sections.len() as isize) as usize;
        self.active_section = sections[next_index];
        self.sync_selection();
        true
    }

    pub fn move_selection(&mut self, step: isize) -> bool {
        let visible = self.visible_snippet_ids();
        if visible.is_empty() {
            self.selected_snippet_id = None;
            return false;
        }

        let current_index = self
            .selected_snippet_id
            .as_ref()
            .and_then(|id| visible.iter().position(|candidate| candidate == id))
            .unwrap_or(0);
        let next_index =
            (current_index as isize + step).rem_euclid(visible.len() as isize) as usize;
        self.selected_snippet_id = Some(visible[next_index].clone());
        true
    }

    #[cfg(test)]
    pub fn set_search_query(&mut self, query: impl Into<String>) {
        self.search_query = query.into();
        self.sync_selection();
    }

    pub fn append_search_query(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        self.search_query.push_str(text);
        self.sync_selection();
    }

    pub fn pop_search_query(&mut self) -> bool {
        if self.search_query.is_empty() {
            return false;
        }

        self.search_query.pop();
        self.sync_selection();
        true
    }

    pub fn clear_search_query(&mut self) -> bool {
        if self.search_query.is_empty() {
            return false;
        }

        self.search_query.clear();
        self.sync_selection();
        true
    }

    pub fn toggle_pinned_only(&mut self) -> bool {
        self.pinned_only = !self.pinned_only;
        self.sync_selection();
        self.pinned_only
    }

    pub fn toggle_selected_pinned(&mut self) -> Option<bool> {
        let selected_id = self.selected_snippet_id.clone()?;
        let snippet = self
            .snippets
            .iter_mut()
            .find(|snippet| snippet.id == selected_id)?;
        snippet.pinned = !snippet.pinned;
        let pinned = snippet.pinned;
        match snippet.section {
            SnippetSection::History => self.persist_history(),
            SnippetSection::Template => self.persist_templates(),
        }
        self.sync_selection();
        Some(pinned)
    }

    pub fn create_template_item(
        &mut self,
        title: String,
        content: String,
        note: Option<String>,
        tags: Vec<String>,
        pinned: bool,
    ) -> Option<String> {
        if title.trim().is_empty() || content.trim().is_empty() {
            return None;
        }

        let new_id = format!("template-{}", Uuid::new_v4().simple());
        let cleaned_note = note.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        let summary = template_summary(cleaned_note.as_deref(), &content);
        let new_item = SnippetRecord {
            id: new_id.clone(),
            section: SnippetSection::Template,
            kind_label: template_kind_label(&content).to_string(),
            title: title.trim().to_string(),
            summary,
            content: content.trim_end().to_string(),
            note: cleaned_note,
            tags,
            pinned,
            source: "手動作成".into(),
            created_label: "たった今".into(),
            captured_at_epoch_secs: None,
        };
        let insert_at = self
            .snippets
            .iter()
            .position(|snippet| snippet.section == SnippetSection::Template)
            .unwrap_or(self.snippets.len());
        self.snippets.insert(insert_at, new_item);
        self.selected_snippet_id = Some(new_id.clone());
        self.sync_selection();
        self.persist_templates();
        Some(new_id)
    }

    pub fn update_template_item(
        &mut self,
        id: &str,
        title: String,
        content: String,
        note: Option<String>,
        tags: Vec<String>,
        favorite: bool,
    ) -> bool {
        let Some(snippet) = self.snippets.iter_mut().find(|s| s.id == id) else {
            return false;
        };
        let cleaned_note = note.and_then(|v| {
            let t = v.trim();
            (!t.is_empty()).then(|| t.to_string())
        });
        snippet.title = title.trim().to_string();
        snippet.summary = template_summary(cleaned_note.as_deref(), &content);
        snippet.content = content.trim_end().to_string();
        snippet.note = cleaned_note;
        snippet.tags = tags;
        snippet.pinned = favorite;
        snippet.kind_label = template_kind_label(&snippet.content).to_string();
        self.persist_templates();
        true
    }

    pub fn ingest_clipboard_capture(&mut self, capture: ClipboardCapture) -> bool {
        if capture.content.trim().is_empty() {
            return false;
        }

        let existing_index = self.snippets.iter().position(|snippet| {
            snippet.section == SnippetSection::History
                && snippet.kind_label == capture.kind_label
                && snippet.content == capture.content
        });
        let (id, pinned) = if let Some(existing_index) = existing_index {
            let existing = self.snippets.remove(existing_index);
            (existing.id, existing.pinned)
        } else {
            (format!("history-{}", Uuid::new_v4().simple()), false)
        };

        let history_record = SnippetRecord {
            id,
            section: SnippetSection::History,
            kind_label: capture.kind_label,
            title: capture.title,
            summary: capture.summary,
            content: capture.content,
            note: capture.note,
            tags: capture.tags,
            pinned,
            source: capture.source,
            created_label: capture.created_label,
            captured_at_epoch_secs: Some(capture.captured_at_epoch_secs),
        };

        let insert_at = self
            .snippets
            .iter()
            .position(|snippet| snippet.section == SnippetSection::History)
            .unwrap_or(0);
        self.snippets.insert(insert_at, history_record);
        self.trim_history_items();
        self.sync_selection();
        self.persist_history();
        true
    }

    pub fn clear_history(&mut self) -> bool {
        let original_len = self.snippets.len();
        self.snippets
            .retain(|snippet| snippet.section != SnippetSection::History);
        if self.snippets.len() == original_len {
            let _ = clear_history_store();
            return false;
        }

        self.selected_snippet_id = None;
        self.sync_selection();
        let _ = clear_history_store();
        true
    }

    pub fn remove_selected(&mut self) -> bool {
        let Some(selected_id) = self.selected_snippet_id.clone() else {
            return false;
        };
        let visible_before = self.visible_snippet_ids();
        let fallback_index = visible_before
            .iter()
            .position(|snippet_id| *snippet_id == selected_id)
            .unwrap_or(0);
        let removed = self
            .snippets
            .iter()
            .find(|snippet| snippet.id == selected_id)
            .map(|snippet| snippet.section);
        let original_len = self.snippets.len();
        self.snippets.retain(|snippet| snippet.id != selected_id);
        if self.snippets.len() == original_len {
            return false;
        }

        let visible_after = self.visible_snippet_ids();
        self.selected_snippet_id = visible_after
            .get(fallback_index)
            .cloned()
            .or_else(|| visible_after.last().cloned());
        self.sync_selection();
        match removed {
            Some(SnippetSection::History) => self.persist_history(),
            Some(SnippetSection::Template) => self.persist_templates(),
            None => {}
        }
        true
    }

    fn trim_history_items(&mut self) {
        while self.section_total_count(SnippetSection::History) > HISTORY_LIMIT {
            let removable_index = self
                .snippets
                .iter()
                .enumerate()
                .rev()
                .find(|(_, snippet)| snippet.section == SnippetSection::History && !snippet.pinned)
                .map(|(index, _)| index)
                .or_else(|| {
                    self.snippets
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, snippet)| snippet.section == SnippetSection::History)
                        .map(|(index, _)| index)
                });

            let Some(removable_index) = removable_index else {
                break;
            };
            self.snippets.remove(removable_index);
        }
    }

    fn visible_snippet_ids(&self) -> Vec<String> {
        self.visible_snippets()
            .into_iter()
            .map(|snippet| snippet.id.clone())
            .collect()
    }

    fn persist_history(&self) {
        if let Err(err) = save_history_records(
            &self
                .snippets
                .iter()
                .filter(|snippet| snippet.section == SnippetSection::History)
                .cloned()
                .collect::<Vec<_>>(),
        ) {
            log::warn!("Failed to save clipboard history: {}", err);
        }
    }

    fn persist_templates(&self) {
        if let Err(err) = save_template_records(
            &self
                .snippets
                .iter()
                .filter(|snippet| snippet.section == SnippetSection::Template)
                .cloned()
                .collect::<Vec<_>>(),
        ) {
            log::warn!("Failed to save snippet templates: {}", err);
        }
    }

    fn sync_selection(&mut self) {
        let visible = self.visible_snippet_ids();
        self.selected_snippet_id = match (self.selected_snippet_id.clone(), visible.first()) {
            (_, None) => None,
            (Some(selected), _) if visible.contains(&selected) => Some(selected),
            (_, Some(first)) => Some(first.clone()),
        };
    }
}

pub fn filter_snippets<'a>(snippets: &'a [SnippetRecord], query: &str) -> Vec<&'a SnippetRecord> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return snippets.iter().collect();
    }

    let lowered = trimmed.to_lowercase();
    snippets
        .iter()
        .filter(|snippet| {
            snippet.title.to_lowercase().contains(&lowered)
                || snippet.summary.to_lowercase().contains(&lowered)
                || snippet.content.to_lowercase().contains(&lowered)
                || snippet
                    .note
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&lowered)
                || snippet.source.to_lowercase().contains(&lowered)
                || snippet
                    .tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains(&lowered))
        })
        .collect()
}

fn history_store_path() -> PathBuf {
    #[cfg(test)]
    {
        return std::env::temp_dir()
            .join("zwg-terminal-tests")
            .join(format!("{}-{}.json", HISTORY_FILE_NAME, std::process::id()));
    }

    #[cfg(not(test))]
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("zwg-terminal")
        .join(HISTORY_FILE_NAME)
}

fn template_store_path() -> PathBuf {
    #[cfg(test)]
    {
        return std::env::temp_dir()
            .join("zwg-terminal-tests")
            .join(format!(
                "{}-{}.json",
                TEMPLATE_FILE_NAME,
                std::process::id()
            ));
    }

    #[cfg(not(test))]
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("zwg-terminal")
        .join(TEMPLATE_FILE_NAME)
}

fn load_history_records() -> Vec<SnippetRecord> {
    let path = history_store_path();
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(store) = serde_json::from_str::<PersistedHistory>(&contents) else {
        return Vec::new();
    };
    store
        .records
        .into_iter()
        .filter(|record| record.section == SnippetSection::History)
        .collect()
}

fn load_template_records() -> Vec<SnippetRecord> {
    let path = template_store_path();
    let Ok(contents) = fs::read_to_string(path) else {
        return default_template_records();
    };
    let Ok(store) = serde_json::from_str::<PersistedHistory>(&contents) else {
        return default_template_records();
    };
    store
        .records
        .into_iter()
        .filter(|record| record.section == SnippetSection::Template)
        .collect::<Vec<_>>()
}

fn save_history_records(records: &[SnippetRecord]) -> std::io::Result<()> {
    let path = history_store_path();
    if records.is_empty() {
        return clear_history_store();
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let store = PersistedHistory {
        version: HISTORY_STORE_VERSION,
        records: records.to_vec(),
    };
    let contents = serde_json::to_string_pretty(&store)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    fs::write(path, contents)
}

fn save_template_records(records: &[SnippetRecord]) -> std::io::Result<()> {
    let path = template_store_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let store = PersistedHistory {
        version: TEMPLATE_STORE_VERSION,
        records: records.to_vec(),
    };
    let contents = serde_json::to_string_pretty(&store)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    fs::write(path, contents)
}

fn clear_history_store() -> std::io::Result<()> {
    let path = history_store_path();
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn default_template_records() -> Vec<SnippetRecord> {
    vec![
        SnippetRecord {
            id: "template-weekly".into(),
            section: SnippetSection::Template,
            kind_label: "TEXT".into(),
            title: "週次メモの雛形".into(),
            summary: "決定事項、課題、次週の 3 ブロックに絞った簡易ノートです。".into(),
            content: "## Weekly Update\n\n### Done\n- \n\n### Risks\n- \n\n### Next\n- ".into(),
            note: Some("CopyQ のメモ運用向けに短文化。".into()),
            tags: vec!["notes".into(), "weekly".into()],
            pinned: true,
            source: "Editor".into(),
            created_label: "2026年3月17日 09:10".into(),
            captured_at_epoch_secs: None,
        },
        SnippetRecord {
            id: "template-followup".into(),
            section: SnippetSection::Template,
            kind_label: "TEXT".into(),
            title: "打ち合わせ後のフォロー".into(),
            summary: "議事録共有前に送る軽いフォロー文です。".into(),
            content: "本日はありがとうございました。\n議事メモを整理して別途共有します。\n先に認識差があればこの返信で教えてください。".into(),
            note: None,
            tags: vec!["meeting".into(), "follow-up".into()],
            pinned: false,
            source: "Mail".into(),
            created_label: "2026年3月16日 14:22".into(),
            captured_at_epoch_secs: None,
        },
        SnippetRecord {
            id: "template-build".into(),
            section: SnippetSection::Template,
            kind_label: "CODE".into(),
            title: "ZWG ビルド確認".into(),
            summary: "変更後の標準ビルド確認セットです。".into(),
            content: "cargo fmt --all\ncargo test -p zwg\ncargo build -p zwg\ncargo build -p zwg --release".into(),
            note: Some("このリポジトリの作業後検証に合わせたコマンド群。".into()),
            tags: vec!["cargo".into(), "validation".into()],
            pinned: true,
            source: "Terminal".into(),
            created_label: "2026年3月18日 19:02".into(),
            captured_at_epoch_secs: None,
        },
    ]
}

fn format_relative_epoch_secs(epoch_secs: u64) -> String {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let elapsed_minutes = now_secs.saturating_sub(epoch_secs) / 60;
    match elapsed_minutes {
        0..=59 => format!("{}分前", elapsed_minutes.max(1)),
        60..=1439 => format!("{}時間前", elapsed_minutes / 60),
        _ => format!("{}日前", elapsed_minutes / 1440),
    }
}

fn template_summary(note: Option<&str>, content: &str) -> String {
    if let Some(note) = note.map(str::trim).filter(|note| !note.is_empty()) {
        return truncate_summary(note, 72);
    }

    let flattened = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    truncate_summary(&flattened, 72)
}

fn truncate_summary(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let truncated = trimmed.chars().take(max_chars).collect::<String>();
    format!("{}…", truncated.trim_end())
}

fn template_kind_label(content: &str) -> &'static str {
    let trimmed = content.trim();
    if trimmed.contains('{')
        || trimmed.contains("=>")
        || trimmed.contains("fn ")
        || trimmed.contains("function ")
        || trimmed.contains("cargo ")
    {
        "CODE"
    } else {
        "TEXT"
    }
}

#[cfg(test)]
fn sample_history_records() -> Vec<SnippetRecord> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    vec![
        SnippetRecord {
            id: "history-copyq".into(),
            section: SnippetSection::History,
            kind_label: "TEXT".into(),
            title: "CopyQはクリップボード管理ツールです。".into(),
            summary: "テキスト、画像、その他のデータをコピーすると、自動的に履歴に保存されます。".into(),
            content: "CopyQはクリップボード管理ツールです。テキスト、画像、その他のデータをコピーすると、自動的に履歴に保存されます。".into(),
            note: Some("Safari でコピーした説明文のサンプル。".into()),
            tags: vec!["browser".into()],
            pinned: true,
            source: "Safari".into(),
            created_label: "2026年3月18日 09:46:34".into(),
            captured_at_epoch_secs: Some(now.saturating_sub(5 * 60)),
        },
        SnippetRecord {
            id: "history-code".into(),
            section: SnippetSection::History,
            kind_label: "CODE".into(),
            title: "function fibonacci(n) { if (n <= 1) return n; }".into(),
            summary: "return fibonacci(n - 1)…".into(),
            content: "function fibonacci(n) {\n  if (n <= 1) return n;\n  return fibonacci(n - 1) + fibonacci(n - 2);\n}".into(),
            note: Some("VS Code からコピーしたコード断片のサンプル。".into()),
            tags: vec!["code".into(), "javascript".into()],
            pinned: false,
            source: "VS Code".into(),
            created_label: "2026年3月18日 09:36:34".into(),
            captured_at_epoch_secs: Some(now.saturating_sub(15 * 60)),
        },
        SnippetRecord {
            id: "history-mail".into(),
            section: SnippetSection::History,
            kind_label: "MAIL".into(),
            title: "hello@example.com".into(),
            summary: "".into(),
            content: "hello@example.com".into(),
            note: Some("メールアドレスの履歴サンプル。".into()),
            tags: vec!["mail".into()],
            pinned: false,
            source: "Mail".into(),
            created_label: "2026年3月18日 09:21:34".into(),
            captured_at_epoch_secs: Some(now.saturating_sub(30 * 60)),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{SnippetPaletteModel, SnippetSection, filter_snippets};
    use crate::clipboard_monitor::ClipboardCapture;

    #[test]
    fn palette_model_selects_first_visible_snippet_by_default() {
        let model = SnippetPaletteModel::with_test_data();

        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("history-copyq")
        );
        assert_eq!(model.active_section(), SnippetSection::History);
        assert_eq!(model.visible_count(), 3);
    }

    #[test]
    fn filter_snippets_matches_summary_note_source_content_and_tags() {
        let model = SnippetPaletteModel::with_test_data();
        let snippets = model.snippets();

        assert_eq!(filter_snippets(snippets, "browser").len(), 1);
        assert_eq!(filter_snippets(snippets, "VS Code").len(), 1);
        assert_eq!(filter_snippets(snippets, "weekly").len(), 1);
        assert_eq!(filter_snippets(snippets, "javascript").len(), 1);
        assert_eq!(filter_snippets(snippets, "fibonacci").len(), 1);
    }

    #[test]
    fn switching_sections_reselects_first_visible_item() {
        let mut model = SnippetPaletteModel::with_test_data();

        assert!(model.select_section(SnippetSection::Template));
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("template-weekly")
        );
    }

    #[test]
    fn switching_sections_restores_first_visible_item_in_target_section() {
        let mut model = SnippetPaletteModel::with_test_data();

        assert!(model.select_section(SnippetSection::Template));
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("template-weekly")
        );
        assert!(model.cycle_sections(1));
        assert_eq!(model.active_section(), SnippetSection::History);
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("history-copyq")
        );
    }

    #[test]
    fn toggling_pinned_filter_keeps_visible_selection_valid() {
        let mut model = SnippetPaletteModel::with_test_data();

        assert!(model.select("history-code"));
        assert!(model.toggle_pinned_only());
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("history-copyq")
        );
    }

    #[test]
    fn toggling_selected_pinned_updates_selected_item() {
        let mut model = SnippetPaletteModel::with_test_data();
        assert!(model.select_section(SnippetSection::Template));
        assert!(model.select("template-followup"));

        assert_eq!(model.toggle_selected_pinned(), Some(true));
        assert!(model.selected_snippet().unwrap().pinned);
        assert_eq!(model.toggle_selected_pinned(), Some(false));
        assert!(!model.selected_snippet().unwrap().pinned);
    }

    #[test]
    fn remove_selected_promotes_next_visible_snippet() {
        let mut model = SnippetPaletteModel::with_test_data();

        assert!(model.remove_selected());
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("history-code")
        );
    }

    #[test]
    fn search_query_filters_and_retargets_selection() {
        let mut model = SnippetPaletteModel::with_test_data();

        model.set_search_query("VS Code");

        assert_eq!(model.visible_count(), 1);
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("history-code")
        );
    }

    #[test]
    fn create_new_item_selects_inserted_record() {
        let mut model = SnippetPaletteModel::with_test_data();
        assert!(model.select_section(SnippetSection::Template));

        let new_id = model.create_template_item(
            "新規".into(),
            "text".into(),
            Some("text".into()),
            vec!["memo".into()],
            false,
        );

        assert_eq!(new_id.as_deref(), Some("template-new-4"));
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("template-new-4")
        );
        assert_eq!(
            model
                .selected_snippet()
                .map(|snippet| snippet.summary.as_str()),
            Some("text")
        );
    }

    #[test]
    fn ingest_clipboard_capture_reorders_duplicates_without_growing_history() {
        let mut model = SnippetPaletteModel::with_test_data();
        let original_history_count = model.section_total_count(SnippetSection::History);
        let duplicate_capture = ClipboardCapture {
            sequence_number: 2,
            kind_label: "TEXT".into(),
            title: "CopyQはクリップボード管理ツールです。".into(),
            summary: "テキスト、画像、その他のデータをコピーすると、自動的に履歴に保存されます。".into(),
            content: "CopyQはクリップボード管理ツールです。テキスト、画像、その他のデータをコピーすると、自動的に履歴に保存されます。".into(),
            note: None,
            tags: vec!["browser".into()],
            source: "ZWG Terminal".into(),
            created_label: "2026年3月18日 10:00:00".into(),
            captured_at_epoch_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        assert!(model.ingest_clipboard_capture(duplicate_capture));
        assert_eq!(
            model.section_total_count(SnippetSection::History),
            original_history_count
        );
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("history-copyq")
        );
        assert_eq!(model.snippets()[0].source, "ZWG Terminal");
    }

    #[test]
    fn relative_created_label_formats_minutes_hours_and_days() {
        let model = SnippetPaletteModel::with_test_data();
        let snippets = model.snippets();

        assert_eq!(
            snippets[0].relative_created_label().as_deref(),
            Some("5分前")
        );
        assert_eq!(
            snippets[1].relative_created_label().as_deref(),
            Some("15分前")
        );
        assert_eq!(
            snippets[2].relative_created_label().as_deref(),
            Some("30分前")
        );
        assert_eq!(snippets[3].relative_created_label(), None);
    }

    #[test]
    fn clear_history_retains_templates() {
        let mut model = SnippetPaletteModel::with_test_data();

        assert!(model.clear_history());
        assert_eq!(model.section_total_count(SnippetSection::History), 0);
        assert_eq!(model.section_total_count(SnippetSection::Template), 3);
    }
}
