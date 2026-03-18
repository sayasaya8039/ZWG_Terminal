#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetTab {
    pub id: String,
    pub title: String,
    pub section: SnippetSection,
    pub shortcut_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetRecord {
    pub id: String,
    pub tab_id: String,
    pub kind_label: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub note: Option<String>,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub source: String,
    pub created_label: String,
    pub captured_minutes_ago: Option<u32>,
}

impl SnippetRecord {
    pub fn relative_created_label(&self) -> Option<String> {
        self.captured_minutes_ago.map(format_relative_minutes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetPaletteModel {
    tabs: Vec<SnippetTab>,
    snippets: Vec<SnippetRecord>,
    active_section: SnippetSection,
    selected_snippet_id: Option<String>,
    search_query: String,
    pinned_only: bool,
}

impl SnippetPaletteModel {
    pub fn new() -> Self {
        let tabs = demo_tabs();
        let snippets = demo_snippets();

        let mut model = Self {
            tabs,
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

    pub fn section_total_count(&self, section: SnippetSection) -> usize {
        self.snippets
            .iter()
            .filter(|snippet| {
                self.tab_for_id(&snippet.tab_id)
                    .map(|tab| tab.section == section)
                    .unwrap_or(false)
            })
            .count()
    }

    pub fn section_pinned_count(&self, section: SnippetSection) -> usize {
        self.snippets
            .iter()
            .filter(|snippet| {
                snippet.pinned
                    && self
                        .tab_for_id(&snippet.tab_id)
                        .map(|tab| tab.section == section)
                        .unwrap_or(false)
            })
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
            .filter(|snippet| {
                self.tab_for_id(&snippet.tab_id)
                    .map(|tab| tab.section == self.active_section)
                    .unwrap_or(false)
            })
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
        self.sync_selection();
        Some(pinned)
    }

    pub fn create_new_item(&mut self) -> Option<String> {
        let target_tab = self
            .tabs
            .iter()
            .find(|tab| tab.section == self.active_section)?
            .id
            .clone();
        let section_key = match self.active_section {
            SnippetSection::History => "history",
            SnippetSection::Template => "template",
        };
        let next_index = self
            .snippets
            .iter()
            .filter(|snippet| {
                self.tab_for_id(&snippet.tab_id)
                    .map(|tab| tab.section == self.active_section)
                    .unwrap_or(false)
            })
            .count()
            + 1;
        let new_id = format!("{section_key}-new-{next_index}");
        let new_item = match self.active_section {
            SnippetSection::History => SnippetRecord {
                id: new_id.clone(),
                tab_id: target_tab,
                kind_label: "TEXT".into(),
                title: "検索".into(),
                summary: "Safari でコピーした新規履歴".into(),
                content: "text".into(),
                note: None,
                tags: Vec::new(),
                pinned: false,
                source: "Safari".into(),
                created_label: "たった今".into(),
                captured_minutes_ago: Some(0),
            },
            SnippetSection::Template => SnippetRecord {
                id: new_id.clone(),
                tab_id: target_tab,
                kind_label: "TEXT".into(),
                title: "新規".into(),
                summary: "text".into(),
                content: "text".into(),
                note: None,
                tags: Vec::new(),
                pinned: false,
                source: "手動作成".into(),
                created_label: "たった今".into(),
                captured_minutes_ago: None,
            },
        };
        let insert_at = self
            .snippets
            .iter()
            .position(|snippet| {
                self.tab_for_id(&snippet.tab_id)
                    .map(|tab| tab.section == self.active_section)
                    .unwrap_or(false)
            })
            .unwrap_or(self.snippets.len());
        self.snippets.insert(insert_at, new_item);
        self.selected_snippet_id = Some(new_id.clone());
        self.sync_selection();
        Some(new_id)
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
        true
    }

    fn visible_snippet_ids(&self) -> Vec<String> {
        self.visible_snippets()
            .into_iter()
            .map(|snippet| snippet.id.clone())
            .collect()
    }

    fn tab_for_id(&self, tab_id: &str) -> Option<&SnippetTab> {
        self.tabs.iter().find(|tab| tab.id == tab_id)
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

fn demo_tabs() -> Vec<SnippetTab> {
    vec![
        SnippetTab {
            id: "clipboard".into(),
            title: "Clipboard".into(),
            section: SnippetSection::History,
            shortcut_hint: Some("Alt+C".into()),
        },
        SnippetTab {
            id: "notes".into(),
            title: "Notes".into(),
            section: SnippetSection::Template,
            shortcut_hint: Some("Alt+N".into()),
        },
        SnippetTab {
            id: "commands".into(),
            title: "Commands".into(),
            section: SnippetSection::Template,
            shortcut_hint: Some("Alt+M".into()),
        },
        SnippetTab {
            id: "links".into(),
            title: "Links".into(),
            section: SnippetSection::History,
            shortcut_hint: Some("Alt+L".into()),
        },
    ]
}

fn demo_snippets() -> Vec<SnippetRecord> {
    vec![
        SnippetRecord {
            id: "clipboard-release-mail".into(),
            tab_id: "clipboard".into(),
            kind_label: "TEXT".into(),
            title: "CopyQはクリップボード管理ツールです。テキスト、画像、その他のデータをコピーすると…".into(),
            summary: "テキスト、画像、その他のデータをコピーすると、自動的に履歴に保存されます。".into(),
            content: "CopyQはクリップボード管理ツールです。テキスト、画像、その他のデータをコピーすると、自動的に履歴に保存されます。".into(),
            note: Some("Safari でコピーした説明文のサンプル。".into()),
            tags: vec!["browser".into()],
            pinned: true,
            source: "Safari".into(),
            created_label: "2026年3月18日 09:46:34".into(),
            captured_minutes_ago: Some(5),
        },
        SnippetRecord {
            id: "clipboard-bug-template".into(),
            tab_id: "clipboard".into(),
            kind_label: "CODE".into(),
            title: "function fibonacci(n) { if (n <= 1) return n; return fibonacci(n - 1)…".into(),
            summary: "return n; return fibonacci(n - 1)…".into(),
            content: "function fibonacci(n) {\n  if (n <= 1) return n;\n  return fibonacci(n - 1) + fibonacci(n - 2);\n}".into(),
            note: Some("VS Code からコピーしたコード断片のサンプル。".into()),
            tags: vec!["code".into(), "javascript".into()],
            pinned: false,
            source: "VS Code".into(),
            created_label: "2026年3月18日 09:36:34".into(),
            captured_minutes_ago: Some(15),
        },
        SnippetRecord {
            id: "notes-weekly".into(),
            tab_id: "notes".into(),
            kind_label: "TEXT".into(),
            title: "週次メモの雛形".into(),
            summary: "決定事項、課題、次週の 3 ブロックに絞った簡易ノートです。".into(),
            content: "## Weekly Update\n\n### Done\n- \n\n### Risks\n- \n\n### Next\n- ".into(),
            note: Some("CopyQ のメモ運用向けに短文化。".into()),
            tags: vec!["notes".into(), "weekly".into()],
            pinned: true,
            source: "Editor".into(),
            created_label: "2026年3月17日 09:10".into(),
            captured_minutes_ago: None,
        },
        SnippetRecord {
            id: "notes-followup".into(),
            tab_id: "notes".into(),
            kind_label: "TEXT".into(),
            title: "打ち合わせ後のフォロー".into(),
            summary: "議事録共有前に送る軽いフォロー文です。".into(),
            content: "本日はありがとうございました。\n議事メモを整理して別途共有します。\n先に認識差があればこの返信で教えてください。".into(),
            note: None,
            tags: vec!["meeting".into(), "follow-up".into()],
            pinned: false,
            source: "Mail".into(),
            created_label: "2026年3月16日 14:22".into(),
            captured_minutes_ago: None,
        },
        SnippetRecord {
            id: "commands-build".into(),
            tab_id: "commands".into(),
            kind_label: "CODE".into(),
            title: "ZWG ビルド確認".into(),
            summary: "変更後の標準ビルド確認セットです。".into(),
            content: "cargo fmt --all\ncargo test -p zwg\ncargo build -p zwg\ncargo build -p zwg --release".into(),
            note: Some("このリポジトリの作業後検証に合わせたコマンド群。".into()),
            tags: vec!["cargo".into(), "validation".into()],
            pinned: true,
            source: "Terminal".into(),
            created_label: "2026年3月18日 19:02".into(),
            captured_minutes_ago: None,
        },
        SnippetRecord {
            id: "commands-review".into(),
            tab_id: "commands".into(),
            kind_label: "CODE".into(),
            title: "差分確認".into(),
            summary: "作業前後の状態を崩さず確認するための定番コマンドです。".into(),
            content: "git status --short\ngit diff -- crates/zwg-app/src/app.rs\nrg -n \"snippet|clipboard|copyq\" crates/zwg-app/src".into(),
            note: Some("品質レビュー前の基本セット。".into()),
            tags: vec!["git".into(), "review".into()],
            pinned: false,
            source: "Terminal".into(),
            created_label: "2026年3月18日 18:55".into(),
            captured_minutes_ago: None,
        },
        SnippetRecord {
            id: "links-copyq".into(),
            tab_id: "links".into(),
            kind_label: "MAIL".into(),
            title: "hello@example.com".into(),
            summary: "".into(),
            content: "hello@example.com".into(),
            note: Some("メールアドレスの履歴サンプル。".into()),
            tags: vec!["mail".into()],
            pinned: true,
            source: "Mail".into(),
            created_label: "2026年3月18日 09:21:34".into(),
            captured_minutes_ago: Some(30),
        },
        SnippetRecord {
            id: "links-figma".into(),
            tab_id: "links".into(),
            kind_label: "HTML".into(),
            title: "<div class=\"container\"><h1>Hello World</h1><p>This is a sample…".into(),
            summary: "".into(),
            content: "<div class=\"container\">\n  <h1>Hello World</h1>\n  <p>This is a sample page.</p>\n</div>".into(),
            note: Some("HTML 断片の履歴サンプル。".into()),
            tags: vec!["html".into()],
            pinned: false,
            source: "Chrome".into(),
            created_label: "2026年3月18日 08:51:34".into(),
            captured_minutes_ago: Some(60),
        },
        SnippetRecord {
            id: "clipboard-calendar".into(),
            tab_id: "clipboard".into(),
            kind_label: "DATE".into(),
            title: "2026年3月16日（月）".into(),
            summary: "".into(),
            content: "2026年3月16日（月）".into(),
            note: Some("カレンダーの履歴サンプル。".into()),
            tags: vec!["date".into()],
            pinned: false,
            source: "Calendar".into(),
            created_label: "2026年3月18日 08:46:34".into(),
            captured_minutes_ago: Some(60),
        },
        SnippetRecord {
            id: "clipboard-sql".into(),
            tab_id: "clipboard".into(),
            kind_label: "SQL".into(),
            title: "SELECT users.name, orders.total FROM users INNER JOIN orders…".into(),
            summary: "".into(),
            content: "SELECT users.name, orders.total\nFROM users\nINNER JOIN orders ON orders.user_id = users.id;".into(),
            note: Some("DataGrip からコピーしたクエリ。".into()),
            tags: vec!["sql".into()],
            pinned: false,
            source: "DataGrip".into(),
            created_label: "2026年3月18日 07:46:34".into(),
            captured_minutes_ago: Some(120),
        },
        SnippetRecord {
            id: "clipboard-url".into(),
            tab_id: "clipboard".into(),
            kind_label: "LINK".into(),
            title: "https://www.example.com/article/how-clipboard-managers-work".into(),
            summary: "".into(),
            content: "https://www.example.com/article/how-clipboard-managers-work".into(),
            note: Some("URL の履歴サンプル。".into()),
            tags: vec!["link".into()],
            pinned: false,
            source: "Safari".into(),
            created_label: "2026年3月18日 07:16:34".into(),
            captured_minutes_ago: Some(150),
        },
    ]
}

fn format_relative_minutes(minutes: u32) -> String {
    match minutes {
        0..=59 => format!("{minutes}分前"),
        60..=1439 => format!("{}時間前", minutes / 60),
        _ => format!("{}日前", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::{SnippetPaletteModel, SnippetSection, filter_snippets};

    #[test]
    fn palette_model_selects_first_visible_snippet_by_default() {
        let model = SnippetPaletteModel::new();

        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("clipboard-release-mail")
        );
        assert_eq!(model.active_section(), SnippetSection::History);
        assert_eq!(model.visible_count(), 7);
    }

    #[test]
    fn filter_snippets_matches_summary_note_source_content_and_tags() {
        let model = SnippetPaletteModel::new();
        let snippets = model.snippets();

        assert_eq!(filter_snippets(snippets, "validation").len(), 1);
        assert_eq!(filter_snippets(snippets, "DataGrip").len(), 1);
        assert_eq!(filter_snippets(snippets, "議事").len(), 1);
        assert_eq!(filter_snippets(snippets, "javascript").len(), 1);
        assert_eq!(filter_snippets(snippets, "fibonacci").len(), 1);
    }

    #[test]
    fn switching_sections_reselects_first_visible_item() {
        let mut model = SnippetPaletteModel::new();

        assert!(model.select_section(SnippetSection::Template));
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("notes-weekly")
        );
    }

    #[test]
    fn switching_sections_restores_first_visible_item_in_target_section() {
        let mut model = SnippetPaletteModel::new();

        assert!(model.select_section(SnippetSection::Template));
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("notes-weekly")
        );
        assert!(model.cycle_sections(1));
        assert_eq!(model.active_section(), SnippetSection::History);
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("clipboard-release-mail")
        );
    }

    #[test]
    fn toggling_pinned_filter_keeps_visible_selection_valid() {
        let mut model = SnippetPaletteModel::new();

        assert!(model.select("clipboard-bug-template"));
        assert!(model.toggle_pinned_only());
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("clipboard-release-mail")
        );
    }

    #[test]
    fn toggling_selected_pinned_updates_selected_item() {
        let mut model = SnippetPaletteModel::new();
        assert!(model.select_section(SnippetSection::Template));
        assert!(model.select("commands-review"));

        assert_eq!(model.toggle_selected_pinned(), Some(true));
        assert!(model.selected_snippet().unwrap().pinned);
        assert_eq!(model.toggle_selected_pinned(), Some(false));
        assert!(!model.selected_snippet().unwrap().pinned);
    }

    #[test]
    fn remove_selected_promotes_next_visible_snippet() {
        let mut model = SnippetPaletteModel::new();

        assert!(model.remove_selected());
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("clipboard-bug-template")
        );
    }

    #[test]
    fn search_query_filters_and_retargets_selection() {
        let mut model = SnippetPaletteModel::new();

        model.set_search_query("VS Code");

        assert_eq!(model.visible_count(), 1);
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("clipboard-bug-template")
        );
    }

    #[test]
    fn create_new_item_selects_inserted_record() {
        let mut model = SnippetPaletteModel::new();

        let new_id = model.create_new_item();

        assert_eq!(new_id.as_deref(), Some("history-new-8"));
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("history-new-8")
        );
        assert_eq!(
            model
                .selected_snippet()
                .map(|snippet| snippet.source.as_str()),
            Some("Safari")
        );
    }

    #[test]
    fn relative_created_label_formats_minutes_hours_and_days() {
        let model = SnippetPaletteModel::new();
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
            snippets[9].relative_created_label().as_deref(),
            Some("2時間前")
        );
        assert_eq!(snippets[2].relative_created_label(), None);
    }
}
