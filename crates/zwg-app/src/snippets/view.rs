use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::snippets::{CsvEncoding, Snippet, SnippetStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnippetQueueMode {
    Off,
    Fifo,
    Lifo,
}

/// Minimal palette state for browsing saved snippets.
#[derive(Debug, Clone)]
pub struct SnippetPalette {
    store: SnippetStore,
    visible: bool,
    query: String,
    active_group: Option<String>,
    selected_store_index: Option<usize>,
    queue_mode: SnippetQueueMode,
    queue: VecDeque<String>,
    ignore_case: bool,
    min_text_length: usize,
    exclude_patterns: Vec<String>,
    favorites_only: bool,
}

impl SnippetPalette {
    pub fn load() -> Self {
        Self::new(SnippetStore::load())
    }

    pub fn new(store: SnippetStore) -> Self {
        let mut palette = Self {
            store,
            visible: false,
            query: String::new(),
            active_group: None,
            selected_store_index: None,
            queue_mode: SnippetQueueMode::Off,
            queue: VecDeque::new(),
            ignore_case: true,
            min_text_length: 1,
            exclude_patterns: Vec::new(),
            favorites_only: false,
        };
        palette.sync_selection();
        palette
    }

    #[allow(dead_code)]
    pub fn store(&self) -> &SnippetStore {
        &self.store
    }

    #[allow(dead_code)]
    pub fn into_store(self) -> SnippetStore {
        self.store
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    #[allow(dead_code)]
    pub fn show(&mut self) {
        self.visible = true;
        self.sync_selection();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.favorites_only = false;
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            self.sync_selection();
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.sync_selection();
    }

    pub fn clear_query(&mut self) {
        if self.query.is_empty() {
            return;
        }

        self.query.clear();
        self.sync_selection();
    }

    pub fn groups(&self) -> &[String] {
        self.store.groups()
    }

    pub fn group_name_at(&self, index: usize) -> Option<&str> {
        self.store.group_at(index)
    }

    pub fn group_index_by_name(&self, name: &str) -> Option<usize> {
        self.store.group_index_by_name(name)
    }

    pub fn store_path(&self) -> &Path {
        self.store.path()
    }

    pub fn csv_path(&self) -> PathBuf {
        SnippetStore::csv_path()
    }

    pub fn active_group(&self) -> Option<&str> {
        self.active_group.as_deref()
    }

    pub fn active_group_label(&self) -> String {
        self.active_group
            .clone()
            .unwrap_or_else(|| "all".to_string())
            .to_lowercase()
    }

    pub fn set_active_group(&mut self, group: Option<String>) {
        self.active_group = group;
        self.sync_selection();
    }

    pub fn set_favorites_only(&mut self, favorites_only: bool) {
        self.favorites_only = favorites_only;
        self.sync_selection();
    }

    pub fn set_filter_preferences(
        &mut self,
        ignore_case: bool,
        min_text_length: usize,
        exclude_patterns: Vec<String>,
    ) {
        self.ignore_case = ignore_case;
        self.min_text_length = min_text_length.max(1);
        self.exclude_patterns = exclude_patterns;
        self.sync_selection();
    }

    pub fn cycle_group(&mut self, forward: bool) {
        let mut groups: Vec<Option<String>> = vec![None];
        groups.extend(self.store.groups().iter().cloned().map(Some));

        let current_index = groups
            .iter()
            .position(|candidate| candidate.as_deref() == self.active_group())
            .unwrap_or(0);
        let next_index = if forward {
            (current_index + 1) % groups.len()
        } else if current_index == 0 {
            groups.len().saturating_sub(1)
        } else {
            current_index - 1
        };

        self.active_group = groups[next_index].clone();
        self.sync_selection();
    }

    pub fn create_group(&mut self) -> bool {
        let mut suffix = 1usize;
        loop {
            let candidate = if suffix == 1 {
                "New Group".to_string()
            } else {
                format!("New Group {suffix}")
            };

            match self.store.add_group(candidate.clone()) {
                Ok(Some(group)) => {
                    self.active_group = Some(group);
                    self.sync_selection();
                    return true;
                }
                Ok(None) => {
                    suffix += 1;
                }
                Err(error) => {
                    log::warn!("Failed to create snippet group: {}", error);
                    return false;
                }
            }
        }
    }

    pub fn add_group_named(&mut self, name: impl Into<String>) -> bool {
        match self.store.add_group(name.into()) {
            Ok(Some(group)) => {
                self.active_group = Some(group);
                self.sync_selection();
                true
            }
            Ok(None) => false,
            Err(error) => {
                log::warn!("Failed to add snippet group: {}", error);
                false
            }
        }
    }

    pub fn rename_group(&mut self, index: usize, name: impl Into<String>) -> bool {
        let Some(previous_group) = self.store.group_at(index).map(str::to_string) else {
            return false;
        };
        match self.store.rename_group(index, name.into()) {
            Ok(true) => {
                if self
                    .active_group
                    .as_deref()
                    .map(|group| group.eq_ignore_ascii_case(&previous_group))
                    .unwrap_or(false)
                {
                    self.active_group = self.store.group_at(index).map(str::to_string);
                }
                self.sync_selection();
                true
            }
            Ok(false) => false,
            Err(error) => {
                log::warn!("Failed to rename snippet group: {}", error);
                false
            }
        }
    }

    pub fn move_group_up(&mut self, index: usize) -> Option<usize> {
        match self.store.move_group_up(index) {
            Ok(next_index) => next_index,
            Err(error) => {
                log::warn!("Failed to move snippet group up: {}", error);
                None
            }
        }
    }

    pub fn move_group_down(&mut self, index: usize) -> Option<usize> {
        match self.store.move_group_down(index) {
            Ok(next_index) => next_index,
            Err(error) => {
                log::warn!("Failed to move snippet group down: {}", error);
                None
            }
        }
    }

    pub fn delete_group(&mut self, index: usize) -> bool {
        let Some(removed_name) = self.store.group_at(index).map(str::to_string) else {
            return false;
        };
        match self.store.delete_group(index) {
            Ok(true) => {
                if self
                    .active_group
                    .as_deref()
                    .map(|group| group.eq_ignore_ascii_case(&removed_name))
                    .unwrap_or(false)
                {
                    self.active_group = None;
                }
                self.sync_selection();
                true
            }
            Ok(false) => false,
            Err(error) => {
                log::warn!("Failed to delete snippet group: {}", error);
                false
            }
        }
    }

    pub fn create_snippet_from_query(&mut self) -> bool {
        let query = self.query.trim();
        let (title, content) = if query.is_empty() {
            (
                "New Snippet".to_string(),
                "echo \"Edit me in %APPDATA%\\\\zwg\\\\snippets.json\"".to_string(),
            )
        } else {
            (query.to_string(), query.to_string())
        };

        match self
            .store
            .add_snippet(title, content, self.active_group.as_deref())
        {
            Ok(Some(index)) => {
                self.selected_store_index = Some(index);
                self.sync_selection();
                true
            }
            Ok(None) => false,
            Err(error) => {
                log::warn!("Failed to create snippet: {}", error);
                false
            }
        }
    }

    pub fn delete_selected(&mut self) -> bool {
        let Some(index) = self.selected_store_index else {
            return false;
        };

        match self.store.delete_snippet(index) {
            Ok(true) => {
                self.sync_selection();
                true
            }
            Ok(false) => false,
            Err(error) => {
                log::warn!("Failed to delete snippet: {}", error);
                false
            }
        }
    }

    pub fn add_snippet(
        &mut self,
        title: impl Into<String>,
        content: impl Into<String>,
        group: Option<&str>,
    ) -> Option<usize> {
        match self.store.add_snippet(title, content, group) {
            Ok(index) => {
                if let Some(index) = index {
                    self.selected_store_index = Some(index);
                    self.sync_selection();
                }
                index
            }
            Err(error) => {
                log::warn!("Failed to add snippet: {}", error);
                None
            }
        }
    }

    pub fn update_snippet(
        &mut self,
        index: usize,
        title: impl Into<String>,
        content: impl Into<String>,
        group: Option<&str>,
    ) -> bool {
        match self.store.update_snippet(index, title, content, group) {
            Ok(updated) => {
                self.selected_store_index = Some(index);
                self.sync_selection();
                updated
            }
            Err(error) => {
                log::warn!("Failed to update snippet: {}", error);
                false
            }
        }
    }

    pub fn move_snippet_up(&mut self, index: usize) -> Option<usize> {
        match self.store.move_snippet_up(index) {
            Ok(next_index) => {
                if let Some(next_index) = next_index {
                    self.selected_store_index = Some(next_index);
                }
                self.sync_selection();
                next_index
            }
            Err(error) => {
                log::warn!("Failed to move snippet up: {}", error);
                None
            }
        }
    }

    pub fn move_snippet_down(&mut self, index: usize) -> Option<usize> {
        match self.store.move_snippet_down(index) {
            Ok(next_index) => {
                if let Some(next_index) = next_index {
                    self.selected_store_index = Some(next_index);
                }
                self.sync_selection();
                next_index
            }
            Err(error) => {
                log::warn!("Failed to move snippet down: {}", error);
                None
            }
        }
    }

    pub fn move_snippet_to_group(&mut self, index: usize, target_group: &str) -> bool {
        match self.store.move_snippet_to_group(index, target_group) {
            Ok(updated) => {
                self.selected_store_index = Some(index);
                self.sync_selection();
                updated
            }
            Err(error) => {
                log::warn!("Failed to move snippet to group: {}", error);
                false
            }
        }
    }

    pub fn delete_snippet(&mut self, index: usize) -> bool {
        match self.store.delete_snippet(index) {
            Ok(deleted) => {
                if deleted {
                    self.sync_selection();
                }
                deleted
            }
            Err(error) => {
                log::warn!("Failed to delete snippet: {}", error);
                false
            }
        }
    }

    pub fn clear_all(&mut self) -> bool {
        match self.store.clear_all() {
            Ok(()) => {
                self.sync_selection();
                true
            }
            Err(error) => {
                log::warn!("Failed to clear snippets: {}", error);
                false
            }
        }
    }

    pub fn toggle_favorite(&mut self, index: usize) -> Option<bool> {
        match self.store.toggle_favorite(index) {
            Ok(is_favorite) => {
                if is_favorite.is_some() {
                    self.selected_store_index = Some(index);
                    self.sync_selection();
                }
                is_favorite
            }
            Err(error) => {
                log::warn!("Failed to toggle snippet favorite: {}", error);
                None
            }
        }
    }

    pub fn snippets_for_group(&self, group: Option<&str>) -> Vec<(usize, &Snippet)> {
        self.store.snippets_for_group(group)
    }

    #[allow(dead_code)]
    pub fn filtered_items(&self) -> Vec<&Snippet> {
        self.filtered_indices()
            .into_iter()
            .map(|index| &self.store.items()[index])
            .collect()
    }

    pub fn filtered_entries(&self) -> Vec<(usize, &Snippet)> {
        self.filtered_indices()
            .into_iter()
            .map(|index| (index, &self.store.items()[index]))
            .collect()
    }

    pub fn selected_filtered_index(&self) -> Option<usize> {
        let selected = self.selected_store_index?;
        self.filtered_indices()
            .iter()
            .position(|index| *index == selected)
    }

    pub fn selected_item(&self) -> Option<&Snippet> {
        self.selected_store_index
            .and_then(|index| self.store.items().get(index))
    }

    pub fn selected_content(&self) -> Option<&str> {
        self.selected_item().map(|snippet| snippet.content.as_str())
    }

    pub fn queue_mode(&self) -> SnippetQueueMode {
        self.queue_mode
    }

    pub fn queue_status_label(&self) -> String {
        match self.queue_mode {
            SnippetQueueMode::Off => "FIFO: OFF".to_string(),
            SnippetQueueMode::Fifo => format!("FIFO: ON ({})", self.queue.len()),
            SnippetQueueMode::Lifo => format!("LIFO: ON ({})", self.queue.len()),
        }
    }

    pub fn toggle_fifo_mode(&mut self) {
        self.queue_mode = if self.queue_mode == SnippetQueueMode::Fifo {
            self.queue.clear();
            SnippetQueueMode::Off
        } else {
            SnippetQueueMode::Fifo
        };
    }

    pub fn toggle_lifo_mode(&mut self) {
        self.queue_mode = if self.queue_mode == SnippetQueueMode::Lifo {
            self.queue.clear();
            SnippetQueueMode::Off
        } else {
            SnippetQueueMode::Lifo
        };
    }

    pub fn activate_selected(&mut self) -> Option<String> {
        let content = self.selected_content()?.to_string();
        self.dispatch_content(content)
    }

    pub fn activate_store_index(&mut self, index: usize) -> Option<String> {
        self.select_store_index(index);
        self.activate_selected()
    }

    pub fn dequeue(&mut self) -> Option<String> {
        match self.queue_mode {
            SnippetQueueMode::Off => None,
            SnippetQueueMode::Fifo => self.queue.pop_front(),
            SnippetQueueMode::Lifo => self.queue.pop_back(),
        }
    }

    pub fn set_queue_mode(&mut self, mode: SnippetQueueMode) {
        self.queue_mode = mode;
        if mode == SnippetQueueMode::Off {
            self.queue.clear();
        }
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn export_csv(&self) -> std::io::Result<PathBuf> {
        self.store.export_csv()
    }

    pub fn import_csv(&mut self) -> std::io::Result<usize> {
        let count = self.store.import_csv()?;
        self.sync_selection();
        Ok(count)
    }

    pub fn set_fifo_mode(&mut self) {
        self.queue_mode = SnippetQueueMode::Fifo;
    }

    pub fn set_lifo_mode(&mut self) {
        self.queue_mode = SnippetQueueMode::Lifo;
    }

    pub fn export_csv_with_encoding(
        &self,
        path: &Path,
        encoding: CsvEncoding,
    ) -> std::io::Result<()> {
        self.store.export_csv_with_encoding(path, encoding)
    }

    pub fn import_csv_with_encoding(
        &mut self,
        path: &Path,
        encoding: CsvEncoding,
        clear_before_import: bool,
    ) -> std::io::Result<usize> {
        let count = self
            .store
            .import_csv_with_encoding(path, encoding, clear_before_import)?;
        self.sync_selection();
        Ok(count)
    }

    pub fn select_next(&mut self) {
        self.move_selection(1);
    }

    pub fn select_previous(&mut self) {
        self.move_selection(-1);
    }

    pub fn select_store_index(&mut self, index: usize) {
        self.selected_store_index = Some(index);
        self.sync_selection();
    }

    pub fn item_count(&self) -> usize {
        self.store.items().len()
    }

    pub fn favorite_count(&self) -> usize {
        self.store
            .items()
            .iter()
            .filter(|snippet| snippet.is_favorite)
            .count()
    }

    pub fn reload_from_disk(&mut self) {
        let path = self.store.path().to_path_buf();
        self.store = SnippetStore::load_from_path(path);
        self.sync_selection();
    }

    pub fn item_metric(snippet: &Snippet) -> usize {
        snippet.metric()
    }

    fn dispatch_content(&mut self, content: String) -> Option<String> {
        match self.queue_mode {
            SnippetQueueMode::Off => Some(content),
            SnippetQueueMode::Fifo | SnippetQueueMode::Lifo => {
                self.queue.push_back(content);
                None
            }
        }
    }

    fn move_selection(&mut self, step: isize) {
        let filtered = self.filtered_indices();
        if filtered.is_empty() {
            self.selected_store_index = None;
            return;
        }

        let current = self
            .selected_store_index
            .and_then(|selected| filtered.iter().position(|index| *index == selected))
            .unwrap_or(0);

        let next = (current as isize + step).rem_euclid(filtered.len() as isize) as usize;
        self.selected_store_index = Some(filtered[next]);
    }

    fn sync_selection(&mut self) {
        if let Some(active_group) = &self.active_group {
            let exists = self
                .store
                .groups()
                .iter()
                .any(|group| group.eq_ignore_ascii_case(active_group));
            if !exists {
                self.active_group = None;
            }
        }

        let filtered = self.filtered_indices();
        self.selected_store_index = match (self.selected_store_index, filtered.first()) {
            (_, None) => None,
            (Some(selected), _) if filtered.contains(&selected) => Some(selected),
            (_, Some(first)) => Some(*first),
        };
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.query.trim().to_string();
        let active_group = self.active_group.as_deref();

        self.store
            .items()
            .iter()
            .enumerate()
            .filter_map(|(index, snippet)| {
                let matches_group = active_group
                    .map(|group| snippet.group.eq_ignore_ascii_case(group))
                    .unwrap_or(true);
                let passes_len = snippet.content.trim().chars().count() >= self.min_text_length;
                let passes_patterns = self.exclude_patterns.iter().all(|pattern| {
                    let pattern = pattern.trim();
                    pattern.is_empty() || !snippet.matches_query(pattern, self.ignore_case)
                });
                let passes_favorites = !self.favorites_only || snippet.is_favorite;
                (matches_group
                    && passes_len
                    && passes_patterns
                    && passes_favorites
                    && snippet.matches_query(&query, self.ignore_case))
                .then_some(index)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_filters_items_and_keeps_selection_valid() {
        let store = SnippetStore::from_items(vec![
            Snippet::new("Greeting", "Hello there"),
            Snippet::new("Deploy", "cargo run --release"),
            Snippet::new("Error Template", "Please share the exact error output."),
        ]);
        let mut palette = SnippetPalette::new(store);

        palette.set_query("error");

        let items = palette.filtered_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Error Template");
        assert_eq!(
            palette.selected_content(),
            Some("Please share the exact error output.")
        );
    }

    #[test]
    fn selection_wraps_across_filtered_results() {
        let store =
            SnippetStore::from_items(vec![Snippet::new("One", "1"), Snippet::new("Two", "2")]);
        let mut palette = SnippetPalette::new(store);

        palette.select_previous();
        assert_eq!(palette.selected_filtered_index(), Some(1));

        palette.select_next();
        assert_eq!(palette.selected_filtered_index(), Some(0));
    }

    #[test]
    fn active_group_filters_visible_items() {
        let store = SnippetStore::from_items(vec![
            Snippet::with_group("One", "1", "Alpha"),
            Snippet::with_group("Two", "2", "Beta"),
        ]);
        let mut palette = SnippetPalette::new(store);

        palette.set_active_group(Some("Beta".to_string()));

        let items = palette.filtered_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Two");
    }

    #[test]
    fn fifo_mode_queues_and_dequeues_in_order() {
        let store =
            SnippetStore::from_items(vec![Snippet::new("One", "A"), Snippet::new("Two", "B")]);
        let mut palette = SnippetPalette::new(store);

        palette.toggle_fifo_mode();
        assert_eq!(palette.activate_selected(), None);
        palette.select_next();
        assert_eq!(palette.activate_selected(), None);

        assert_eq!(palette.dequeue(), Some("A".to_string()));
        assert_eq!(palette.dequeue(), Some("B".to_string()));
    }

    #[test]
    fn activate_store_index_dispatches_clicked_item() {
        let store =
            SnippetStore::from_items(vec![Snippet::new("One", "A"), Snippet::new("Two", "B")]);
        let mut palette = SnippetPalette::new(store);

        assert_eq!(palette.activate_store_index(1), Some("B".to_string()));
        assert_eq!(palette.selected_item().map(|item| item.title.as_str()), Some("Two"));
    }

    #[test]
    fn toggling_favorite_updates_selected_item() {
        let store = SnippetStore::from_items(vec![Snippet::new("One", "A")]);
        let mut palette = SnippetPalette::new(store);

        assert_eq!(palette.toggle_favorite(0), Some(true));
        assert!(palette.selected_item().unwrap().is_favorite);
        assert_eq!(palette.toggle_favorite(0), Some(false));
        assert!(!palette.selected_item().unwrap().is_favorite);
    }
}
