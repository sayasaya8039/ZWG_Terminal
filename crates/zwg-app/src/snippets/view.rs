use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::snippets::{Snippet, SnippetStore};

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

    pub fn export_csv(&self) -> std::io::Result<PathBuf> {
        self.store.export_csv()
    }

    pub fn import_csv(&mut self) -> std::io::Result<usize> {
        let count = self.store.import_csv()?;
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
        let query = self.query.trim().to_lowercase();
        let active_group = self.active_group.as_deref();

        self.store
            .items()
            .iter()
            .enumerate()
            .filter_map(|(index, snippet)| {
                let matches_group = active_group
                    .map(|group| snippet.group.eq_ignore_ascii_case(group))
                    .unwrap_or(true);
                (matches_group && snippet.matches_query(&query)).then_some(index)
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
}
