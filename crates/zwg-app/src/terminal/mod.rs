//! Terminal module — PTY, screen buffer, VT parser, GPUI view

pub mod pty;
pub mod surface;
pub mod view;
#[cfg(not(feature = "ghostty_vt"))]
pub mod vt_parser;

#[cfg(not(feature = "ghostty_vt"))]
use std::collections::VecDeque;

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
            text: String::new(),
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

// Catppuccin Mocha defaults
pub const DEFAULT_FG: u32 = 0xcdd6f4;
pub const DEFAULT_BG: u32 = 0x1e1e2e;

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
            let x = self.cursor.x as usize;

            // Pad to cursor position
            if line.text.is_ascii() {
                while line.text.len() <= x {
                    line.text.push(' ');
                }
            } else {
                while line.text.chars().count() <= x {
                    line.text.push(' ');
                }
            }

            // ASCII fast path
            if line.text.is_ascii() && ch.is_ascii() && x < line.text.len() {
                unsafe {
                    line.text.as_bytes_mut()[x] = ch as u8;
                }
            } else {
                let byte_start = line
                    .text
                    .char_indices()
                    .nth(x)
                    .map(|(i, _)| i)
                    .unwrap_or(line.text.len());
                let byte_end = line
                    .text
                    .char_indices()
                    .nth(x + 1)
                    .map(|(i, _)| i)
                    .unwrap_or(line.text.len());
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                line.text.replace_range(byte_start..byte_end, s);
            }

            line.is_dirty = true;
            self.generation = self.generation.wrapping_add(1);
            if let Some(rg) = self.row_generations.get_mut(y) {
                *rg = self.generation;
            }
            self.cursor.x += 1;
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
                self.scrollback.pop_front();
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
}

// Re-export main types
pub use surface::TerminalSurface;
pub use view::TerminalPane;
