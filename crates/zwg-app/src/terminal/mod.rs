//! Terminal module — PTY, screen buffer, VT parser, GPUI view

#[cfg(feature = "ghostty_vt")]
mod gpu_view;
mod grid_renderer;
#[cfg(all(feature = "ghostty_vt", target_os = "windows"))]
mod native_gpu_presenter;
pub mod pty;
pub(crate) mod simd_ops;
pub mod surface;
pub mod view;
#[cfg(not(feature = "ghostty_vt"))]
pub mod vt_parser;
pub(crate) mod win32_input;

use std::sync::{Arc, atomic::AtomicBool};

#[derive(Debug, Clone)]
pub struct TerminalSettings {
    pub cols: u16,
    pub rows: u16,
    pub scrollback_lines: usize,
    pub font_family: String,
    pub font_size: f32,
    pub cursor_blink: bool,
    pub copy_on_select: bool,
    pub gpu_acceleration: bool,
    pub fg_color: u32,
    pub bg_color: u32,
    pub background_image_path: Option<String>,
    pub background_image_opacity: f32,
    pub global_hotkeys: Vec<String>,
    pub input_suppressed: Arc<AtomicBool>,
}

#[cfg(not(feature = "ghostty_vt"))]
use std::collections::VecDeque;
#[cfg(not(feature = "ghostty_vt"))]
use unicode_width::UnicodeWidthChar;

/// Style information for a run of characters
#[cfg(not(feature = "ghostty_vt"))]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StyleSpan {
    pub len: u16,
    pub fg_color: u32,
    pub bg_color: u32,
    pub flags: u8,
}

#[cfg(not(feature = "ghostty_vt"))]
impl StyleSpan {
    pub fn is_bold(&self) -> bool {
        self.flags & 0x01 != 0
    }
    pub fn is_italic(&self) -> bool {
        self.flags & 0x02 != 0
    }
    pub fn is_underline(&self) -> bool {
        self.flags & 0x04 != 0
    }
    pub fn is_inverse(&self) -> bool {
        self.flags & 0x08 != 0
    }
    pub fn is_strikethrough(&self) -> bool {
        self.flags & 0x10 != 0
    }
}

/// A single terminal line with text and style runs
#[cfg(not(feature = "ghostty_vt"))]
#[derive(Debug, Clone)]
pub struct TerminalLine {
    pub text: String,
    pub styles: Vec<StyleSpan>,
    pub is_dirty: bool,
    pub is_wrapped: bool,
}

#[cfg(not(feature = "ghostty_vt"))]
impl TerminalLine {
    pub fn new() -> Self {
        Self {
            // Perf: pre-allocate typical line capacity (80 cols)
            text: String::with_capacity(80),
            styles: vec![StyleSpan {
                len: 0,
                fg_color: DEFAULT_FG,
                bg_color: DEFAULT_BG,
                flags: 0,
            }],
            is_dirty: true,
            is_wrapped: false,
        }
    }
}

/// Cursor state
#[cfg(not(feature = "ghostty_vt"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorState {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    pub shape: CursorShape,
}

#[cfg(not(feature = "ghostty_vt"))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

// Terminal defaults
pub const DEFAULT_FG: u32 = 0xE5E5EA;
pub const DEFAULT_BG: u32 = 0x000000;

/// Screen buffer holding viewport + scrollback
#[cfg(not(feature = "ghostty_vt"))]
pub struct ScreenBuffer {
    pub viewport: VecDeque<TerminalLine>,
    pub scrollback: VecDeque<TerminalLine>,
    pub cols: u16,
    pub rows: u16,
    pub cursor: CursorState,
    pub scroll_offset: usize,
    pub max_scrollback: usize,
    pub current_fg: u32,
    pub current_bg: u32,
    pub current_flags: u8,
    pub alternate_screen: Option<VecDeque<TerminalLine>>,
    pub title: Option<String>,
    pub generation: u64,
    pub row_generations: Vec<u64>,
    /// Path to RAM disk scrollback file for this terminal (if available)
    ramdisk_scrollback_path: Option<std::path::PathBuf>,
    /// Number of lines spilled to RAM disk
    ramdisk_spilled_lines: usize,
}

#[cfg(not(feature = "ghostty_vt"))]
impl ScreenBuffer {
    pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Self {
        Self {
            viewport: (0..rows).map(|_| TerminalLine::new()).collect(),
            scrollback: VecDeque::new(),
            cols,
            rows,
            cursor: CursorState {
                visible: true,
                ..Default::default()
            },
            scroll_offset: 0,
            max_scrollback,
            current_fg: DEFAULT_FG,
            current_bg: DEFAULT_BG,
            current_flags: 0,
            alternate_screen: None,
            title: None,
            generation: 1,
            row_generations: vec![1u64; rows as usize],
            ramdisk_scrollback_path: std::env::var("ZWG_RAMDISK")
                .ok()
                .map(|rd| {
                    let dir = std::path::PathBuf::from(&rd).join("scrollback");
                    let _ = std::fs::create_dir_all(&dir);
                    dir.join(format!("{}.scrollback", uuid::Uuid::new_v4()))
                }),
            ramdisk_spilled_lines: 0,
        }
    }

    pub fn write_char(&mut self, ch: char) {
        if self.cursor.y >= self.rows {
            self.scroll_up();
            self.cursor.y = self.rows - 1;
        }
        let y = self.cursor.y as usize;
        if y < self.viewport.len() {
            let line = &mut self.viewport[y];
            // C2: cursor.x is in cell columns; convert to char index
            let char_idx = cell_to_char_index(&line.text, self.cursor.x as usize);

            // H4: compute char count once, then pad incrementally
            let mut current_chars = line.text.chars().count();
            while current_chars <= char_idx {
                line.text.push(' ');
                current_chars += 1;
            }

            // H5: safe string replacement (no unsafe as_bytes_mut)
            let byte_start = line
                .text
                .char_indices()
                .nth(char_idx)
                .map(|(i, _)| i)
                .unwrap_or(line.text.len());
            let byte_end = line
                .text
                .char_indices()
                .nth(char_idx + 1)
                .map(|(i, _)| i)
                .unwrap_or(line.text.len());
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            line.text.replace_range(byte_start..byte_end, s);

            line.is_dirty = true;
            self.generation = self.generation.wrapping_add(1);
            if let Some(rg) = self.row_generations.get_mut(y) {
                *rg = self.generation;
            }
            // C2: advance cursor by unicode display width
            let char_width = ch.width().unwrap_or(1).max(1) as u16;
            self.cursor.x += char_width;
            if self.cursor.x >= self.cols {
                self.cursor.x = 0;
                self.cursor.y += 1;
                line.is_wrapped = true;
            }
        }
    }

    pub fn scroll_up(&mut self) {
        if let Some(line) = self.viewport.pop_front() {
            self.scrollback.push_back(line);
            if self.scrollback.len() > self.max_scrollback {
                if let Some(evicted) = self.scrollback.pop_front() {
                    if let Some(ref path) = self.ramdisk_scrollback_path {
                        use std::io::Write;
                        if let Ok(mut file) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                        {
                            let bytes = evicted.text.as_bytes();
                            let len = bytes.len() as u32;
                            let _ = file.write_all(&len.to_le_bytes());
                            let _ = file.write_all(bytes);
                            self.ramdisk_spilled_lines += 1;
                        }
                    }
                }
            }
            if self.scroll_offset > 0 {
                self.scroll_offset = self
                    .scroll_offset
                    .saturating_add(1)
                    .min(self.scrollback.len());
            }
            self.viewport.push_back(TerminalLine::new());
            self.generation = self.generation.wrapping_add(1);
            for rg in &mut self.row_generations {
                *rg = self.generation;
            }
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        while self.viewport.len() > rows as usize {
            if let Some(line) = self.viewport.pop_front() {
                self.scrollback.push_back(line);
            }
        }
        while self.viewport.len() < rows as usize {
            self.viewport.push_back(TerminalLine::new());
        }
        // M5: trim scrollback to max_scrollback after resize, spilling to RAM disk
        while self.scrollback.len() > self.max_scrollback {
            if let Some(evicted) = self.scrollback.pop_front() {
                if let Some(ref path) = self.ramdisk_scrollback_path {
                    use std::io::Write;
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        let bytes = evicted.text.as_bytes();
                        let len = bytes.len() as u32;
                        let _ = file.write_all(&len.to_le_bytes());
                        let _ = file.write_all(bytes);
                        self.ramdisk_spilled_lines += 1;
                    }
                }
            }
        }
        if self.scroll_offset > self.scrollback.len() {
            self.scroll_offset = self.scrollback.len();
        }
        if self.cursor.x >= cols {
            self.cursor.x = cols.saturating_sub(1);
        }
        if self.cursor.y >= rows {
            self.cursor.y = rows.saturating_sub(1);
        }
        self.generation = self.generation.wrapping_add(1);
        self.row_generations.resize(rows as usize, self.generation);
        for rg in &mut self.row_generations {
            *rg = self.generation;
        }
    }

    pub fn visible_line(&self, row: usize) -> Option<&TerminalLine> {
        let rows = self.rows as usize;
        if row >= rows {
            return None;
        }
        let total = self.scrollback.len() + self.viewport.len();
        let clamped_offset = self.scroll_offset.min(total);
        let end = total.saturating_sub(clamped_offset);
        let start = end.saturating_sub(rows);
        let abs_idx = start + row;
        if abs_idx >= end || abs_idx >= total {
            return None;
        }
        if abs_idx < self.scrollback.len() {
            self.scrollback.get(abs_idx)
        } else {
            self.viewport.get(abs_idx - self.scrollback.len())
        }
    }

    pub fn scroll_viewport(&mut self, delta_lines: i32) {
        if delta_lines == 0 {
            return;
        }

        let max_offset = self.scrollback.len();
        if delta_lines > 0 {
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(delta_lines as usize)
                .min(max_offset);
        } else {
            self.scroll_offset = self
                .scroll_offset
                .saturating_sub(delta_lines.unsigned_abs() as usize);
        }

        self.generation = self.generation.wrapping_add(1);
        for rg in &mut self.row_generations {
            *rg = self.generation;
        }
    }

    pub fn clear_history(&mut self) {
        self.scrollback.clear();
        self.scroll_offset = 0;
        self.alternate_screen = None;
        self.generation = self.generation.wrapping_add(1);
        for rg in &mut self.row_generations {
            *rg = self.generation;
        }
    }

    /// Load spilled scrollback lines from RAM disk.
    /// Returns lines in chronological order (oldest first).
    pub fn load_spilled_scrollback(&self, offset: usize, count: usize) -> Vec<String> {
        let path = match &self.ramdisk_scrollback_path {
            Some(p) if p.exists() => p,
            _ => return Vec::new(),
        };

        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        let mut lines = Vec::new();
        let mut pos = 0usize;
        let mut line_idx = 0usize;

        while pos + 4 <= data.len() {
            let len = u32::from_le_bytes(
                data[pos..pos + 4].try_into().unwrap_or([0; 4]),
            ) as usize;
            pos += 4;

            if pos + len > data.len() {
                break;
            }

            if line_idx >= offset && lines.len() < count {
                if let Ok(text) = std::str::from_utf8(&data[pos..pos + len]) {
                    lines.push(text.to_string());
                }
            }

            pos += len;
            line_idx += 1;

            if lines.len() >= count {
                break;
            }
        }

        lines
    }

    /// Total scrollback lines including spilled ones on RAM disk.
    pub fn total_scrollback_lines(&self) -> usize {
        self.scrollback.len() + self.ramdisk_spilled_lines
    }
}

#[cfg(not(feature = "ghostty_vt"))]
impl Drop for ScreenBuffer {
    fn drop(&mut self) {
        if let Some(ref path) = self.ramdisk_scrollback_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Convert a cell column position to a character index in the string.
/// Accounts for wide (CJK) characters occupying 2 cells.
/// SIMD-accelerated: O(1) for ASCII-only text (the common case).
#[cfg(not(feature = "ghostty_vt"))]
fn cell_to_char_index(text: &str, cell_col: usize) -> usize {
    simd_ops::fast_cell_to_char_index(text, cell_col)
}

// Re-export main types
#[allow(unused_imports)]
pub use surface::TerminalSurface;
pub use view::TerminalPane;

#[cfg(all(test, not(feature = "ghostty_vt")))]
mod tests {
    use super::*;

    #[test]
    fn screen_buffer_new_has_correct_dimensions() {
        let sb = ScreenBuffer::new(80, 24, 1000);
        assert_eq!(sb.cols, 80);
        assert_eq!(sb.rows, 24);
        assert_eq!(sb.viewport.len(), 24);
        assert_eq!(sb.scrollback.len(), 0);
        assert_eq!(sb.cursor.x, 0);
        assert_eq!(sb.cursor.y, 0);
        assert!(sb.cursor.visible);
    }

    #[test]
    fn write_char_advances_cursor() {
        let mut sb = ScreenBuffer::new(80, 24, 1000);
        sb.write_char('A');
        assert_eq!(sb.cursor.x, 1);
        assert_eq!(sb.cursor.y, 0);
        assert_eq!(sb.viewport[0].text.trim_end(), "A");
    }

    #[test]
    fn write_char_wraps_at_end_of_line() {
        let mut sb = ScreenBuffer::new(3, 2, 100);
        sb.write_char('A');
        sb.write_char('B');
        sb.write_char('C'); // fills col 0,1,2 → wraps
        assert_eq!(sb.cursor.x, 0);
        assert_eq!(sb.cursor.y, 1);
        assert!(sb.viewport[0].is_wrapped);
    }

    #[test]
    fn write_char_unicode() {
        let mut sb = ScreenBuffer::new(80, 24, 1000);
        sb.write_char('あ');
        sb.write_char('い');
        // C2: CJK chars have width 2, so cursor advances by 2 per char
        assert_eq!(sb.cursor.x, 4);
        let text = sb.viewport[0].text.trim_end();
        assert!(text.contains('あ'));
        assert!(text.contains('い'));
    }

    #[test]
    fn scroll_up_moves_line_to_scrollback() {
        let mut sb = ScreenBuffer::new(80, 2, 100);
        sb.write_char('A');
        sb.scroll_up();
        assert_eq!(sb.scrollback.len(), 1);
        assert_eq!(sb.viewport.len(), 2);
        assert!(sb.scrollback[0].text.contains('A'));
    }

    #[test]
    fn scroll_up_caps_scrollback() {
        let mut sb = ScreenBuffer::new(80, 2, 3);
        for _ in 0..5 {
            sb.scroll_up();
        }
        assert!(sb.scrollback.len() <= 3);
    }

    #[test]
    fn resize_shrink_moves_to_scrollback() {
        let mut sb = ScreenBuffer::new(80, 10, 1000);
        assert_eq!(sb.viewport.len(), 10);
        sb.resize(80, 5);
        assert_eq!(sb.viewport.len(), 5);
        assert_eq!(sb.scrollback.len(), 5);
        assert_eq!(sb.cols, 80);
        assert_eq!(sb.rows, 5);
    }

    #[test]
    fn resize_grow_adds_empty_lines() {
        let mut sb = ScreenBuffer::new(80, 5, 1000);
        sb.resize(80, 10);
        assert_eq!(sb.viewport.len(), 10);
        assert_eq!(sb.rows, 10);
    }

    #[test]
    fn resize_clamps_cursor() {
        let mut sb = ScreenBuffer::new(80, 24, 1000);
        sb.cursor.x = 79;
        sb.cursor.y = 23;
        sb.resize(40, 10);
        assert!(sb.cursor.x < 40);
        assert!(sb.cursor.y < 10);
    }

    #[test]
    fn visible_line_returns_viewport() {
        let mut sb = ScreenBuffer::new(80, 3, 100);
        sb.write_char('X');
        let line = sb.visible_line(0).unwrap();
        assert!(line.text.contains('X'));
    }

    #[test]
    fn visible_line_out_of_range_returns_none() {
        let sb = ScreenBuffer::new(80, 3, 100);
        assert!(sb.visible_line(3).is_none());
        assert!(sb.visible_line(100).is_none());
    }

    #[test]
    fn terminal_line_new_defaults() {
        let line = TerminalLine::new();
        assert!(line.text.is_empty());
        assert!(line.is_dirty);
        assert!(!line.is_wrapped);
        assert_eq!(line.styles.len(), 1);
        assert_eq!(line.styles[0].fg_color, DEFAULT_FG);
        assert_eq!(line.styles[0].bg_color, DEFAULT_BG);
    }

    #[test]
    fn style_span_flags() {
        let span = StyleSpan {
            len: 5,
            fg_color: 0xffffff,
            bg_color: 0x000000,
            flags: 0x01 | 0x04 | 0x10, // bold + underline + strikethrough
        };
        assert!(span.is_bold());
        assert!(!span.is_italic());
        assert!(span.is_underline());
        assert!(!span.is_inverse());
        assert!(span.is_strikethrough());
    }

    #[test]
    fn cursor_shape_default_is_block() {
        let shape = CursorShape::default();
        assert_eq!(shape, CursorShape::Block);
    }
}
