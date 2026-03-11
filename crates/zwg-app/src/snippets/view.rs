use crate::snippets::{Snippet, SnippetStore};

/// Minimal palette state for browsing saved snippets.
#[derive(Debug, Clone)]
pub struct SnippetPalette {
    store: SnippetStore,
    visible: bool,
    query: String,
    selected_store_index: Option<usize>,
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
            selected_store_index: None,
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

    pub fn filtered_items(&self) -> Vec<&Snippet> {
        self.filtered_indices()
            .into_iter()
            .map(|index| &self.store.items()[index])
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

    pub fn select_next(&mut self) {
        self.move_selection(1);
    }

    pub fn select_previous(&mut self) {
        self.move_selection(-1);
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
        let filtered = self.filtered_indices();
        self.selected_store_index = match (self.selected_store_index, filtered.first()) {
            (_, None) => None,
            (Some(selected), _) if filtered.contains(&selected) => Some(selected),
            (_, Some(first)) => Some(*first),
        };
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.query.trim().to_lowercase();

        self.store
            .items()
            .iter()
            .enumerate()
            .filter_map(|(index, snippet)| snippet.matches_query(&query).then_some(index))
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
}
