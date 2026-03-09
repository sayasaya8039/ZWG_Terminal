//! ghostty-vt-sys — Low-level C FFI bindings to Ghostty's VT parser
//!
//! Phase 1: Will contain auto-generated bindings from ghostty_vt.h
//! Currently a placeholder — the actual Ghostty integration requires:
//! 1. vendor/ghostty submodule (git submodule add https://github.com/ghostty-org/ghostty vendor/ghostty)
//! 2. Zig 0.14+ for building libghostty_vt
//! 3. ghostty_vt.h C API header

#![allow(dead_code)]

use std::ffi::c_void;

/// Opaque handle to a Ghostty terminal instance
pub type GhosttyTerminal = *mut c_void;

// Phase 1 FFI declarations (currently stubbed)
// These will be populated when Ghostty submodule is integrated
#[cfg(feature = "ghostty")]
unsafe extern "C" {
    pub fn ghostty_vt_terminal_new(cols: u16, rows: u16) -> GhosttyTerminal;
    pub fn ghostty_vt_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_vt_terminal_feed(terminal: GhosttyTerminal, bytes: *const u8, len: usize);
    pub fn ghostty_vt_terminal_resize(terminal: GhosttyTerminal, cols: u16, rows: u16);
    pub fn ghostty_vt_terminal_dump_viewport_row(
        terminal: GhosttyTerminal,
        row: u16,
    ) -> *const u8;
    pub fn ghostty_vt_terminal_cursor_position(
        terminal: GhosttyTerminal,
        col_out: *mut u16,
        row_out: *mut u16,
    );
}

/// Style run from Ghostty's VT parser (12 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyStyleRun {
    pub start_col: u16,
    pub end_col: u16,
    pub fg_r: u8,
    pub fg_g: u8,
    pub fg_b: u8,
    pub bg_r: u8,
    pub bg_g: u8,
    pub bg_b: u8,
    pub flags: u8,
    pub _reserved: u8,
}
