#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetTab {
    pub id: String,
    pub title: String,
    pub shortcut_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetRecord {
    pub id: String,
    pub tab_id: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub note: Option<String>,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub source: String,
    pub created_label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetPaletteModel {
    tabs: Vec<SnippetTab>,
    snippets: Vec<SnippetRecord>,
    active_tab_id: String,
    selected_snippet_id: Option<String>,
    search_query: String,
    pinned_only: bool,
}

impl SnippetPaletteModel {
    pub fn new() -> Self {
        let tabs = demo_tabs();
        let snippets = demo_snippets();
        let active_tab_id = tabs
            .first()
            .map(|tab| tab.id.clone())
            .unwrap_or_else(|| "clipboard".to_string());

        let mut model = Self {
            tabs,
            snippets,
            active_tab_id,
            selected_snippet_id: None,
            search_query: String::new(),
            pinned_only: false,
        };
        model.sync_selection();
        model
    }

    pub fn tabs(&self) -> &[SnippetTab] {
        &self.tabs
    }

    #[cfg(test)]
    pub fn snippets(&self) -> &[SnippetRecord] {
        &self.snippets
    }

    pub fn active_tab(&self) -> Option<&SnippetTab> {
        self.tabs.iter().find(|tab| tab.id == self.active_tab_id)
    }

    pub fn active_tab_title(&self) -> &str {
        self.active_tab()
            .map(|tab| tab.title.as_str())
            .unwrap_or("Clipboard")
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn pinned_only(&self) -> bool {
        self.pinned_only
    }

    pub fn total_count(&self) -> usize {
        self.snippets.len()
    }

    pub fn pinned_count(&self) -> usize {
        self.snippets
            .iter()
            .filter(|snippet| snippet.pinned)
            .count()
    }

    #[cfg(test)]
    pub fn visible_count(&self) -> usize {
        self.visible_snippets().len()
    }

    pub fn tab_title_for(&self, tab_id: &str) -> Option<&str> {
        self.tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.title.as_str())
    }

    pub fn visible_snippets(&self) -> Vec<&SnippetRecord> {
        filter_snippets(&self.snippets, &self.search_query)
            .into_iter()
            .filter(|snippet| snippet.tab_id == self.active_tab_id)
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

    pub fn select_tab(&mut self, tab_id: &str) -> bool {
        if self.tabs.iter().any(|tab| tab.id == tab_id) {
            self.active_tab_id = tab_id.to_string();
            self.sync_selection();
            return true;
        }

        false
    }

    pub fn cycle_tabs(&mut self, step: isize) -> bool {
        let Some(current_index) = self
            .tabs
            .iter()
            .position(|tab| tab.id == self.active_tab_id)
        else {
            return false;
        };
        let next_index =
            (current_index as isize + step).rem_euclid(self.tabs.len() as isize) as usize;
        let next_id = self.tabs[next_index].id.clone();
        self.active_tab_id = next_id;
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
            shortcut_hint: Some("Alt+C".into()),
        },
        SnippetTab {
            id: "notes".into(),
            title: "Notes".into(),
            shortcut_hint: Some("Alt+N".into()),
        },
        SnippetTab {
            id: "commands".into(),
            title: "Commands".into(),
            shortcut_hint: Some("Alt+M".into()),
        },
        SnippetTab {
            id: "links".into(),
            title: "Links".into(),
            shortcut_hint: Some("Alt+L".into()),
        },
    ]
}

fn demo_snippets() -> Vec<SnippetRecord> {
    vec![
        SnippetRecord {
            id: "clipboard-release-mail".into(),
            tab_id: "clipboard".into(),
            title: "リリース連絡メール".into(),
            summary: "社内向けにそのまま流せる短い完了報告です。".into(),
            content: "本日の配布版を反映しました。\n\n- 反映環境: production\n- 反映時刻: 18:30 JST\n- 監視: 主要導線の疎通確認済み\n\n不具合があればこのスレッドへ返信してください。".into(),
            note: Some("CopyQ の履歴タブを意識して、直近でよく再利用する通知文として配置。".into()),
            tags: vec!["release".into(), "mail".into(), "team".into()],
            pinned: true,
            source: "Clipboard".into(),
            created_label: "2026年3月18日 18:30".into(),
        },
        SnippetRecord {
            id: "clipboard-bug-template".into(),
            tab_id: "clipboard".into(),
            title: "不具合再現依頼".into(),
            summary: "再現手順の回収に使う定型質問です。".into(),
            content: "以下 4 点を共有してください。\n1. 実行したコマンド\n2. 期待した結果\n3. 実際の結果\n4. 可能ならスクリーンショット".into(),
            note: Some("サポート返信の初動で使う。".into()),
            tags: vec!["support".into(), "bug".into()],
            pinned: false,
            source: "Clipboard".into(),
            created_label: "2026年3月18日 17:08".into(),
        },
        SnippetRecord {
            id: "notes-weekly".into(),
            tab_id: "notes".into(),
            title: "週次メモの雛形".into(),
            summary: "決定事項、課題、次週の 3 ブロックに絞った簡易ノートです。".into(),
            content: "## Weekly Update\n\n### Done\n- \n\n### Risks\n- \n\n### Next\n- ".into(),
            note: Some("CopyQ のメモ運用向けに短文化。".into()),
            tags: vec!["notes".into(), "weekly".into()],
            pinned: true,
            source: "Editor".into(),
            created_label: "2026年3月17日 09:10".into(),
        },
        SnippetRecord {
            id: "notes-followup".into(),
            tab_id: "notes".into(),
            title: "打ち合わせ後のフォロー".into(),
            summary: "議事録共有前に送る軽いフォロー文です。".into(),
            content: "本日はありがとうございました。\n議事メモを整理して別途共有します。\n先に認識差があればこの返信で教えてください。".into(),
            note: None,
            tags: vec!["meeting".into(), "follow-up".into()],
            pinned: false,
            source: "Mail".into(),
            created_label: "2026年3月16日 14:22".into(),
        },
        SnippetRecord {
            id: "commands-build".into(),
            tab_id: "commands".into(),
            title: "ZWG ビルド確認".into(),
            summary: "変更後の標準ビルド確認セットです。".into(),
            content: "cargo fmt --all\ncargo test -p zwg\ncargo build -p zwg\ncargo build -p zwg --release".into(),
            note: Some("このリポジトリの作業後検証に合わせたコマンド群。".into()),
            tags: vec!["cargo".into(), "validation".into()],
            pinned: true,
            source: "Terminal".into(),
            created_label: "2026年3月18日 19:02".into(),
        },
        SnippetRecord {
            id: "commands-review".into(),
            tab_id: "commands".into(),
            title: "差分確認".into(),
            summary: "作業前後の状態を崩さず確認するための定番コマンドです。".into(),
            content: "git status --short\ngit diff -- crates/zwg-app/src/app.rs\nrg -n \"snippet|clipboard|copyq\" crates/zwg-app/src".into(),
            note: Some("品質レビュー前の基本セット。".into()),
            tags: vec!["git".into(), "review".into()],
            pinned: false,
            source: "Terminal".into(),
            created_label: "2026年3月18日 18:55".into(),
        },
        SnippetRecord {
            id: "links-copyq".into(),
            tab_id: "links".into(),
            title: "CopyQ Repository".into(),
            summary: "移植元として参照する公式リポジトリ。".into(),
            content: "https://github.com/hluk/CopyQ".into(),
            note: Some("Tabs, items, pinning, search の挙動参照用。".into()),
            tags: vec!["copyq".into(), "reference".into()],
            pinned: true,
            source: "Browser".into(),
            created_label: "2026年3月18日 16:40".into(),
        },
        SnippetRecord {
            id: "links-figma".into(),
            tab_id: "links".into(),
            title: "CopyQ 風 UI Figma".into(),
            summary: "今回のパネル配置と見た目のベース。".into(),
            content: "https://www.figma.com/make/hEbX0XvRtTkJy5s2CpfPf6/CopyQ%E9%A2%A8UI%E4%BD%9C%E6%88%90?t=XHTxOigxfFfGa9if-1".into(),
            note: Some("トップバー配下のアンカード配置を合わせるためのデザインソース。".into()),
            tags: vec!["figma".into(), "ui".into()],
            pinned: false,
            source: "Figma".into(),
            created_label: "2026年3月18日 15:55".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{SnippetPaletteModel, filter_snippets};

    #[test]
    fn palette_model_selects_first_visible_snippet_by_default() {
        let model = SnippetPaletteModel::new();

        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("clipboard-release-mail")
        );
        assert_eq!(model.active_tab_title(), "Clipboard");
    }

    #[test]
    fn filter_snippets_matches_summary_note_source_content_and_tags() {
        let model = SnippetPaletteModel::new();
        let snippets = model.snippets();

        assert_eq!(filter_snippets(snippets, "validation").len(), 1);
        assert_eq!(filter_snippets(snippets, "Figma").len(), 1);
        assert_eq!(filter_snippets(snippets, "議事").len(), 1);
        assert_eq!(filter_snippets(snippets, "support").len(), 1);
        assert_eq!(filter_snippets(snippets, "production").len(), 1);
    }

    #[test]
    fn switching_tabs_reselects_first_visible_item() {
        let mut model = SnippetPaletteModel::new();

        assert!(model.select_tab("commands"));
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("commands-build")
        );

        assert!(model.cycle_tabs(1));
        assert_eq!(model.active_tab_title(), "Links");
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("links-copyq")
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
        assert!(model.select_tab("commands"));
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

        model.set_search_query("support");

        assert_eq!(model.visible_count(), 1);
        assert_eq!(
            model.selected_snippet().map(|snippet| snippet.id.as_str()),
            Some("clipboard-bug-template")
        );
    }
}
