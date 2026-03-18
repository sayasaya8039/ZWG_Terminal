use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImeTextBuffer {
    text: String,
    selection: Range<usize>,
    marked_range: Option<Range<usize>>,
}

impl Default for ImeTextBuffer {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl ImeTextBuffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let len = text.len();
        Self {
            text,
            selection: len..len,
            marked_range: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.marked_range.clone()
    }

    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    pub fn cursor(&self) -> usize {
        self.selection.end
    }

    pub fn set_cursor_to_start(&mut self) -> bool {
        self.set_selection(0..0)
    }

    pub fn set_cursor_to_end(&mut self) -> bool {
        let len = self.text.len();
        self.set_selection(len..len)
    }

    pub fn set_selection(&mut self, range: Range<usize>) -> bool {
        let normalized = normalize_range(&self.text, range);
        if self.selection == normalized {
            return false;
        }
        self.selection = normalized;
        true
    }

    pub fn set_marked_range(&mut self, range: Option<Range<usize>>) -> bool {
        let normalized = range.map(|value| normalize_range(&self.text, value));
        if self.marked_range == normalized {
            return false;
        }
        self.marked_range = normalized;
        true
    }

    pub fn clear_marked_range(&mut self) -> bool {
        self.set_marked_range(None)
    }

    pub fn replace_range(&mut self, range: Range<usize>, text: &str) -> Option<Range<usize>> {
        let range = normalize_range(&self.text, range);
        if range.start > range.end || range.end > self.text.len() {
            return None;
        }

        self.text.replace_range(range.clone(), text);
        let inserted = range.start..range.start + text.len();
        self.selection = inserted.end..inserted.end;
        Some(inserted)
    }

    pub fn replace_selection(&mut self, text: &str) -> Option<Range<usize>> {
        self.replace_range(self.selection.clone(), text)
    }

    pub fn backspace_grapheme(&mut self) -> bool {
        if !is_collapsed(&self.selection) {
            return self.replace_selection("").is_some();
        }

        let cursor = self.cursor();
        if cursor == 0 {
            return false;
        }
        let prev = previous_grapheme_boundary(&self.text, cursor);
        self.replace_range(prev..cursor, "").is_some()
    }

    pub fn delete_forward_grapheme(&mut self) -> bool {
        if !is_collapsed(&self.selection) {
            return self.replace_selection("").is_some();
        }

        let cursor = self.cursor();
        if cursor >= self.text.len() {
            return false;
        }
        let next = next_grapheme_boundary(&self.text, cursor);
        self.replace_range(cursor..next, "").is_some()
    }

    pub fn move_cursor_grapheme(&mut self, direction: isize) -> bool {
        let next_cursor = if !is_collapsed(&self.selection) {
            if direction < 0 {
                self.selection.start
            } else {
                self.selection.end
            }
        } else if direction < 0 {
            previous_grapheme_boundary(&self.text, self.cursor())
        } else {
            next_grapheme_boundary(&self.text, self.cursor())
        };
        self.set_selection(next_cursor..next_cursor)
    }
}

fn is_collapsed(range: &Range<usize>) -> bool {
    range.start == range.end
}

fn clamp_to_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn normalize_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = clamp_to_char_boundary(text, range.start);
    let end = clamp_to_char_boundary(text, range.end);
    if start <= end { start..end } else { end..start }
}

fn previous_grapheme_boundary(text: &str, cursor: usize) -> usize {
    let cursor = clamp_to_char_boundary(text, cursor);
    UnicodeSegmentation::grapheme_indices(text, true)
        .take_while(|(index, _)| *index < cursor)
        .map(|(index, _)| index)
        .last()
        .unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, cursor: usize) -> usize {
    let cursor = clamp_to_char_boundary(text, cursor);
    UnicodeSegmentation::grapheme_indices(text, true)
        .map(|(index, _)| index)
        .find(|index| *index > cursor)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::ImeTextBuffer;

    #[test]
    fn buffer_backspace_removes_single_emoji_cluster() {
        let mut buffer = ImeTextBuffer::new("A👍🏽B");
        buffer.set_cursor_to_end();

        assert!(buffer.backspace_grapheme());
        assert_eq!(buffer.text(), "A👍🏽");
        assert!(buffer.backspace_grapheme());
        assert_eq!(buffer.text(), "A");
    }

    #[test]
    fn buffer_delete_forward_removes_selection_first() {
        let mut buffer = ImeTextBuffer::new("hello");
        buffer.set_selection(1..4);

        assert!(buffer.delete_forward_grapheme());
        assert_eq!(buffer.text(), "ho");
        assert_eq!(buffer.selection(), 1..1);
    }

    #[test]
    fn buffer_moves_cursor_by_grapheme() {
        let mut buffer = ImeTextBuffer::new("Aあ👍🏽B");
        buffer.set_cursor_to_end();

        assert!(buffer.move_cursor_grapheme(-1));
        let after_b = buffer.cursor();
        assert!(buffer.move_cursor_grapheme(-1));
        let after_emoji = buffer.cursor();
        assert!(after_emoji < after_b);
        assert!(buffer.move_cursor_grapheme(1));
        assert_eq!(buffer.cursor(), after_b);
    }
}
