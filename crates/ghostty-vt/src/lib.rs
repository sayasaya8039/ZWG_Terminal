//! ghostty-vt — Safe Rust wrapper around ghostty-vt-sys
//!
//! Phase 1: Will provide a safe Terminal struct wrapping the C FFI
//! Currently a placeholder with the planned API surface

#![allow(dead_code)]

/// Safe wrapper around Ghostty's terminal instance
pub struct Terminal {
    cols: u16,
    rows: u16,
    // Phase 1: inner: ghostty_vt_sys::GhosttyTerminal,
}

impl Terminal {
    /// Create a new terminal instance
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }

    /// Feed raw bytes into the VT parser
    pub fn feed(&mut self, _data: &[u8]) {
        // Phase 1: ghostty_vt_sys::ghostty_vt_terminal_feed(...)
    }

    /// Resize the terminal
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        // Phase 1: ghostty_vt_sys::ghostty_vt_terminal_resize(...)
    }

    /// Get the current terminal dimensions
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }
}
