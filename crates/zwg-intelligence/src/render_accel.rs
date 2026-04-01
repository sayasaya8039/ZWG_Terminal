//! Render acceleration — techniques ported from herm (aduermael/herm).
//!
//! Provides batch-write buffering, dirty-region tracking, viewport culling,
//! and ANSI diff optimization for terminal rendering.
//!
//! # Key optimizations
//!
//! | Technique              | Effect                                      |
//! |------------------------|---------------------------------------------|
//! | Batch write buffer     | Hundreds of syscalls → 1 per frame           |
//! | Dirty region tracking  | Only re-render changed rows                  |
//! | Viewport culling       | Skip off-screen rows entirely                |
//! | ANSI style diff        | Minimize escape sequence output              |
//! | Smart scroll           | Incremental scroll vs full redraw            |

use std::fmt::Write as FmtWrite;

/// ANSI escape sequences used frequently — pre-allocated to avoid allocations.
pub mod ansi {
    pub const RESET: &str = "\x1b[0m";
    pub const CLEAR_SCREEN: &str = "\x1b[H\x1b[2J\x1b[3J";
    pub const CLEAR_LINE: &str = "\x1b[2K";
    pub const HIDE_CURSOR: &str = "\x1b[?25l";
    pub const SHOW_CURSOR: &str = "\x1b[?25h";
    pub const SAVE_CURSOR: &str = "\x1b[s";
    pub const RESTORE_CURSOR: &str = "\x1b[u";
    pub const ALT_SCREEN_ON: &str = "\x1b[?1049h";
    pub const ALT_SCREEN_OFF: &str = "\x1b[?1049l";
    pub const BRACKETED_PASTE_ON: &str = "\x1b[?2004h";
    pub const BRACKETED_PASTE_OFF: &str = "\x1b[?2004l";

    /// Move cursor to row, col (1-based).
    pub fn cursor_to(buf: &mut String, row: u16, col: u16) {
        use std::fmt::Write;
        let _ = write!(buf, "\x1b[{row};{col}H");
    }

    /// Scroll up N lines (content moves up, new blank lines at bottom).
    pub fn scroll_up(buf: &mut String, n: u16) {
        use std::fmt::Write;
        let _ = write!(buf, "\x1b[{n}S");
    }

    /// Scroll down N lines.
    pub fn scroll_down(buf: &mut String, n: u16) {
        use std::fmt::Write;
        let _ = write!(buf, "\x1b[{n}T");
    }

    /// Set foreground color (256-color).
    pub fn fg256(buf: &mut String, color: u8) {
        use std::fmt::Write;
        let _ = write!(buf, "\x1b[38;5;{color}m");
    }

    /// Set background color (256-color).
    pub fn bg256(buf: &mut String, color: u8) {
        use std::fmt::Write;
        let _ = write!(buf, "\x1b[48;5;{color}m");
    }
}

/// A row-level rendering style for ANSI diff optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub fg: Option<u8>,
    pub bg: Option<u8>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl CellStyle {
    /// Emit only the ANSI codes needed to transition from `prev` to `self`.
    /// Returns empty string if styles are identical (zero-cost).
    pub fn diff_from(&self, prev: &CellStyle, buf: &mut String) {
        if self == prev {
            return;
        }

        // If new style is simpler, reset and apply fresh
        let needs_reset = (prev.bold && !self.bold)
            || (prev.italic && !self.italic)
            || (prev.underline && !self.underline)
            || (prev.fg.is_some() && self.fg.is_none())
            || (prev.bg.is_some() && self.bg.is_none());

        if needs_reset {
            buf.push_str(ansi::RESET);
            // Apply all attributes of new style
            self.emit_full(buf);
            return;
        }

        // Incremental diff: only emit what changed
        if self.bold && !prev.bold {
            buf.push_str("\x1b[1m");
        }
        if self.italic && !prev.italic {
            buf.push_str("\x1b[3m");
        }
        if self.underline && !prev.underline {
            buf.push_str("\x1b[4m");
        }
        if self.fg != prev.fg {
            if let Some(c) = self.fg {
                ansi::fg256(buf, c);
            }
        }
        if self.bg != prev.bg {
            if let Some(c) = self.bg {
                ansi::bg256(buf, c);
            }
        }
    }

    /// Emit full ANSI style (after a reset).
    fn emit_full(&self, buf: &mut String) {
        if self.bold {
            buf.push_str("\x1b[1m");
        }
        if self.italic {
            buf.push_str("\x1b[3m");
        }
        if self.underline {
            buf.push_str("\x1b[4m");
        }
        if let Some(c) = self.fg {
            ansi::fg256(buf, c);
        }
        if let Some(c) = self.bg {
            ansi::bg256(buf, c);
        }
    }
}

/// Batch write buffer — accumulates all render output before a single
/// `write_all()` syscall. Reduces I/O overhead dramatically.
///
/// Default capacity: 64 KB (typical terminal frame is 2-20 KB).
pub struct RenderBuffer {
    buf: String,
}

impl RenderBuffer {
    /// Create with default 64 KB capacity.
    pub fn new() -> Self {
        Self {
            buf: String::with_capacity(64 * 1024),
        }
    }

    /// Create with custom capacity.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: String::with_capacity(cap),
        }
    }

    /// Get mutable reference to the inner buffer for direct writes.
    pub fn buf_mut(&mut self) -> &mut String {
        &mut self.buf
    }

    /// Append a string.
    pub fn push_str(&mut self, s: &str) {
        self.buf.push_str(s);
    }

    /// Append a char.
    pub fn push(&mut self, c: char) {
        self.buf.push(c);
    }

    /// Write formatted content.
    pub fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) {
        let _ = self.buf.write_fmt(args);
    }

    /// Flush the buffer to stdout in a single write.
    /// Returns the number of bytes written.
    pub fn flush_to_stdout(&mut self) -> std::io::Result<usize> {
        use std::io::Write;
        let len = self.buf.len();
        if len > 0 {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(self.buf.as_bytes())?;
            stdout.flush()?;
            self.buf.clear();
        }
        Ok(len)
    }

    /// Current buffer size.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Clear without deallocating.
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

impl Default for RenderBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Dirty region tracker — tracks which rows have changed since last render.
/// Only dirty rows are re-drawn, saving significant I/O.
pub struct DirtyTracker {
    /// One bit per row: true = needs redraw.
    dirty: Vec<bool>,
    /// Previous frame content hash per row (for content-based diffing).
    row_hashes: Vec<u64>,
    /// Whether a full redraw is needed (e.g., after resize).
    full_redraw: bool,
}

impl DirtyTracker {
    /// Create for a terminal with `rows` visible lines.
    pub fn new(rows: usize) -> Self {
        Self {
            dirty: vec![true; rows],
            row_hashes: vec![0; rows],
            full_redraw: true,
        }
    }

    /// Resize the tracker (marks everything dirty).
    pub fn resize(&mut self, rows: usize) {
        self.dirty.resize(rows, true);
        self.row_hashes.resize(rows, 0);
        self.full_redraw = true;
    }

    /// Mark a specific row as dirty.
    pub fn mark_dirty(&mut self, row: usize) {
        if row < self.dirty.len() {
            self.dirty[row] = true;
        }
    }

    /// Mark a range of rows as dirty.
    pub fn mark_range_dirty(&mut self, start: usize, end: usize) {
        for row in start..end.min(self.dirty.len()) {
            self.dirty[row] = true;
        }
    }

    /// Mark all rows dirty (e.g., after terminal resize).
    pub fn mark_all_dirty(&mut self) {
        self.full_redraw = true;
        self.dirty.fill(true);
    }

    /// Check if a row needs redrawing.
    pub fn is_dirty(&self, row: usize) -> bool {
        self.full_redraw || self.dirty.get(row).copied().unwrap_or(false)
    }

    /// Update row content hash. Returns true if the row actually changed.
    pub fn update_row(&mut self, row: usize, content_hash: u64) -> bool {
        if row >= self.row_hashes.len() {
            return true;
        }
        if self.row_hashes[row] != content_hash {
            self.row_hashes[row] = content_hash;
            self.dirty[row] = true;
            true
        } else {
            false
        }
    }

    /// Clear all dirty flags after a render pass.
    pub fn clear(&mut self) {
        self.dirty.fill(false);
        self.full_redraw = false;
    }

    /// Count dirty rows (for stats).
    pub fn dirty_count(&self) -> usize {
        if self.full_redraw {
            self.dirty.len()
        } else {
            self.dirty.iter().filter(|&&d| d).count()
        }
    }

    /// Total rows tracked.
    pub fn total_rows(&self) -> usize {
        self.dirty.len()
    }
}

/// Viewport culler — determines which rows from a scrollback buffer
/// are actually visible and need rendering.
pub struct ViewportCuller {
    /// Total rows of content.
    pub total_rows: usize,
    /// Visible terminal height.
    pub visible_rows: usize,
    /// Current scroll offset (0 = bottom, positive = scrolled up).
    pub scroll_offset: usize,
}

impl ViewportCuller {
    pub fn new(total_rows: usize, visible_rows: usize) -> Self {
        Self {
            total_rows,
            visible_rows,
            scroll_offset: 0,
        }
    }

    /// Get the range of content rows that are currently visible.
    /// Returns (start_row, end_row) where end_row is exclusive.
    pub fn visible_range(&self) -> (usize, usize) {
        if self.total_rows <= self.visible_rows {
            return (0, self.total_rows);
        }
        let bottom = self.total_rows.saturating_sub(self.scroll_offset);
        let top = bottom.saturating_sub(self.visible_rows);
        (top, bottom)
    }

    /// Check if a content row index is currently visible.
    pub fn is_visible(&self, row: usize) -> bool {
        let (start, end) = self.visible_range();
        row >= start && row < end
    }

    /// Number of rows that can be scrolled up.
    pub fn max_scroll(&self) -> usize {
        self.total_rows.saturating_sub(self.visible_rows)
    }

    /// Scroll up by N rows (clamped to max).
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = (self.scroll_offset + n).min(self.max_scroll());
    }

    /// Scroll down by N rows (clamped to 0).
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll to bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// Scroll to top.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = self.max_scroll();
    }
}

/// Smart scroll helper — determines whether to use incremental scroll
/// (ANSI scroll regions) or full redraw based on the delta.
pub enum ScrollStrategy {
    /// Use ANSI scroll commands (small deltas, ≤ half screen).
    Incremental { lines: u16, direction: ScrollDir },
    /// Full redraw (large deltas or resize).
    FullRedraw,
    /// No change needed.
    None,
}

#[derive(Debug, Clone, Copy)]
pub enum ScrollDir {
    Up,
    Down,
}

/// Determine optimal scroll strategy.
pub fn scroll_strategy(
    prev_offset: usize,
    new_offset: usize,
    visible_rows: usize,
) -> ScrollStrategy {
    if prev_offset == new_offset {
        return ScrollStrategy::None;
    }
    let delta = if new_offset > prev_offset {
        new_offset - prev_offset
    } else {
        prev_offset - new_offset
    };

    // If delta is more than half the screen, full redraw is cheaper
    if delta > visible_rows / 2 {
        return ScrollStrategy::FullRedraw;
    }

    let direction = if new_offset > prev_offset {
        ScrollDir::Up
    } else {
        ScrollDir::Down
    };

    ScrollStrategy::Incremental {
        lines: delta as u16,
        direction,
    }
}

/// Fast content hash for row comparison (FNV-1a 64-bit).
/// Much faster than SHA/MD5 for this use case.
pub fn row_hash(content: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in content {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// ---------------------------------------------------------------------------
// GPU Glyph Atlas Cache
// ---------------------------------------------------------------------------

/// A CPU-side glyph atlas that packs rasterized glyphs into a texture sheet.
/// The atlas is uploaded to GPU memory once; subsequent frames only reference
/// UV coordinates, eliminating per-frame CPU→GPU glyph transfers.
///
/// Layout: row-major, top-left origin.  Each glyph occupies a fixed cell
/// (`cell_w × cell_h`) so the GPU shader can index directly via
/// `(glyph_index * cell_w, 0)` without per-glyph offset tables.
pub struct GlyphAtlas {
    /// Raw RGBA pixel data (atlas_width × atlas_height × 4).
    pub pixels: Vec<u8>,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    /// Maps (codepoint, flags) → atlas slot index.
    entries: std::collections::HashMap<(u32, u8), u32>,
    /// Next free slot.
    next_slot: u32,
    /// Maximum slots along the X axis.
    slots_per_row: u32,
    /// True when new glyphs were added since last GPU upload.
    pub dirty: bool,
}

impl GlyphAtlas {
    /// Create an atlas sized for `max_glyphs` unique glyphs.
    pub fn new(cell_w: u32, cell_h: u32, max_glyphs: u32) -> Self {
        let slots_per_row = (max_glyphs as f64).sqrt().ceil() as u32;
        let atlas_width = slots_per_row * cell_w;
        let rows_needed = (max_glyphs + slots_per_row - 1) / slots_per_row;
        let atlas_height = rows_needed * cell_h;
        Self {
            pixels: vec![0u8; (atlas_width * atlas_height * 4) as usize],
            atlas_width,
            atlas_height,
            cell_w,
            cell_h,
            entries: std::collections::HashMap::with_capacity(max_glyphs as usize),
            next_slot: 0,
            slots_per_row,
            dirty: true,
        }
    }

    /// Look up or insert a glyph. Returns the slot index.
    /// `rasterize` is called only when the glyph is not yet cached;
    /// it must fill the provided `&mut [u8]` slice (cell_w × cell_h × 4 RGBA).
    pub fn get_or_insert(
        &mut self,
        codepoint: u32,
        flags: u8,
        rasterize: impl FnOnce(u32, u32) -> Vec<u8>,
    ) -> u32 {
        if let Some(&slot) = self.entries.get(&(codepoint, flags)) {
            return slot;
        }
        let slot = self.next_slot;
        self.next_slot += 1;
        let col = slot % self.slots_per_row;
        let row = slot / self.slots_per_row;
        let x0 = col * self.cell_w;
        let y0 = row * self.cell_h;
        let glyph_pixels = rasterize(self.cell_w, self.cell_h);
        // Blit glyph pixels into the atlas.
        for py in 0..self.cell_h {
            let dst_start = ((y0 + py) * self.atlas_width + x0) as usize * 4;
            let src_start = (py * self.cell_w) as usize * 4;
            let row_bytes = self.cell_w as usize * 4;
            if src_start + row_bytes <= glyph_pixels.len()
                && dst_start + row_bytes <= self.pixels.len()
            {
                self.pixels[dst_start..dst_start + row_bytes]
                    .copy_from_slice(&glyph_pixels[src_start..src_start + row_bytes]);
            }
        }
        self.entries.insert((codepoint, flags), slot);
        self.dirty = true;
        slot
    }

    /// UV coordinates for a slot (normalized 0..1).
    pub fn uv_rect(&self, slot: u32) -> (f32, f32, f32, f32) {
        let col = slot % self.slots_per_row;
        let row = slot / self.slots_per_row;
        let u0 = (col * self.cell_w) as f32 / self.atlas_width as f32;
        let v0 = (row * self.cell_h) as f32 / self.atlas_height as f32;
        let u1 = u0 + self.cell_w as f32 / self.atlas_width as f32;
        let v1 = v0 + self.cell_h as f32 / self.atlas_height as f32;
        (u0, v0, u1, v1)
    }

    /// Number of cached glyphs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Mark atlas as clean after GPU upload.
    pub fn mark_uploaded(&mut self) {
        self.dirty = false;
    }
}

// ---------------------------------------------------------------------------
// Dirty Rect / GPU Scissor Optimization
// ---------------------------------------------------------------------------

/// A rectangular damage region in cell coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageRect {
    pub row_start: u16,
    pub row_end: u16,
    pub col_start: u16,
    pub col_end: u16,
}

impl DamageRect {
    pub fn new(row_start: u16, row_end: u16, col_start: u16, col_end: u16) -> Self {
        Self {
            row_start,
            row_end,
            col_start,
            col_end,
        }
    }

    /// Convert cell-coordinate damage rect to pixel-coordinate scissor rect.
    pub fn to_scissor(&self, cell_w: f32, cell_h: f32, origin_x: f32, origin_y: f32) -> ScissorRect {
        ScissorRect {
            left: origin_x + self.col_start as f32 * cell_w,
            top: origin_y + self.row_start as f32 * cell_h,
            right: origin_x + self.col_end as f32 * cell_w,
            bottom: origin_y + self.row_end as f32 * cell_h,
        }
    }

    /// Check if two damage rects overlap (for merging).
    pub fn overlaps(&self, other: &DamageRect) -> bool {
        self.row_start < other.row_end
            && other.row_start < self.row_end
            && self.col_start < other.col_end
            && other.col_start < self.col_end
    }

    /// Merge two rects into a bounding rect.
    pub fn merge(&self, other: &DamageRect) -> DamageRect {
        DamageRect {
            row_start: self.row_start.min(other.row_start),
            row_end: self.row_end.max(other.row_end),
            col_start: self.col_start.min(other.col_start),
            col_end: self.col_end.max(other.col_end),
        }
    }

    /// Area in cells (for cost estimation).
    pub fn area(&self) -> u32 {
        (self.row_end - self.row_start) as u32 * (self.col_end - self.col_start) as u32
    }
}

/// Pixel-coordinate scissor rect for GPU commands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScissorRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl ScissorRect {
    /// Convert to integer RECT suitable for D3D12 RSSetScissorRects.
    pub fn to_d3d12_rect(&self) -> (i32, i32, i32, i32) {
        (
            self.left.floor() as i32,
            self.top.floor() as i32,
            self.right.ceil() as i32,
            self.bottom.ceil() as i32,
        )
    }
}

/// Coalesce a list of damage rects to reduce GPU scissor state changes.
/// Merges overlapping/adjacent rects, then caps at `max_rects`.
/// If too many rects remain, falls back to a single bounding rect.
pub fn coalesce_damage_rects(rects: &[DamageRect], max_rects: usize) -> Vec<DamageRect> {
    if rects.is_empty() {
        return Vec::new();
    }
    if rects.len() == 1 {
        return rects.to_vec();
    }

    let mut sorted: Vec<DamageRect> = rects.to_vec();
    sorted.sort_unstable_by_key(|r| (r.row_start, r.col_start));

    let mut merged: Vec<DamageRect> = vec![sorted[0]];
    for rect in sorted.iter().skip(1) {
        let last = merged.last().unwrap();
        if last.overlaps(rect)
            || (last.row_start == rect.row_start
                && last.row_end == rect.row_end
                && last.col_end >= rect.col_start)
        {
            let m = merged.last().unwrap().merge(rect);
            *merged.last_mut().unwrap() = m;
        } else {
            merged.push(*rect);
        }
    }

    if merged.len() <= max_rects {
        return merged;
    }

    // Too many rects — fall back to bounding box.
    let bounding = merged
        .iter()
        .copied()
        .reduce(|a, b| a.merge(&b))
        .unwrap();
    vec![bounding]
}

/// Convert per-row damage info into merged DamageRects.
/// `damaged_rows` lists row indices; each row spans the full terminal width.
pub fn damage_rects_from_rows(damaged_rows: &[u16], term_cols: u16) -> Vec<DamageRect> {
    if damaged_rows.is_empty() || term_cols == 0 {
        return Vec::new();
    }
    let mut sorted = damaged_rows.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut rects = Vec::with_capacity(sorted.len());
    let mut run_start = sorted[0];
    let mut run_end = sorted[0] + 1;

    for &row in sorted.iter().skip(1) {
        if row == run_end {
            run_end = row + 1;
        } else {
            rects.push(DamageRect::new(run_start, run_end, 0, term_cols));
            run_start = row;
            run_end = row + 1;
        }
    }
    rects.push(DamageRect::new(run_start, run_end, 0, term_cols));
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_buffer_basic() {
        let mut buf = RenderBuffer::new();
        buf.push_str("hello");
        buf.push(' ');
        buf.push_str("world");
        assert_eq!(buf.len(), 11);
        assert!(!buf.is_empty());
        buf.clear();
        assert!(buf.is_empty());
    }

    #[test]
    fn dirty_tracker_basics() {
        let mut dt = DirtyTracker::new(10);
        assert_eq!(dt.dirty_count(), 10); // all dirty initially
        dt.clear();
        assert_eq!(dt.dirty_count(), 0);

        dt.mark_dirty(3);
        dt.mark_dirty(7);
        assert_eq!(dt.dirty_count(), 2);
        assert!(dt.is_dirty(3));
        assert!(dt.is_dirty(7));
        assert!(!dt.is_dirty(0));
    }

    #[test]
    fn dirty_tracker_hash_update() {
        let mut dt = DirtyTracker::new(5);
        dt.clear();

        assert!(dt.update_row(0, 12345));
        assert!(!dt.update_row(0, 12345)); // same hash → not dirty
        assert!(dt.update_row(0, 99999)); // different hash → dirty
    }

    #[test]
    fn viewport_culler_small_content() {
        let vc = ViewportCuller::new(5, 10);
        assert_eq!(vc.visible_range(), (0, 5));
        assert!(vc.is_visible(0));
        assert!(vc.is_visible(4));
    }

    #[test]
    fn viewport_culler_scrollback() {
        let mut vc = ViewportCuller::new(100, 24);
        assert_eq!(vc.visible_range(), (76, 100));
        assert!(vc.is_visible(99));
        assert!(!vc.is_visible(0));

        vc.scroll_up(10);
        assert_eq!(vc.visible_range(), (66, 90));

        vc.scroll_to_top();
        assert_eq!(vc.visible_range(), (0, 24));

        vc.scroll_to_bottom();
        assert_eq!(vc.visible_range(), (76, 100));
    }

    #[test]
    fn scroll_strategy_none() {
        assert!(matches!(scroll_strategy(10, 10, 24), ScrollStrategy::None));
    }

    #[test]
    fn scroll_strategy_incremental() {
        match scroll_strategy(10, 13, 24) {
            ScrollStrategy::Incremental { lines, .. } => assert_eq!(lines, 3),
            _ => panic!("expected incremental"),
        }
    }

    #[test]
    fn scroll_strategy_full_redraw() {
        assert!(matches!(
            scroll_strategy(0, 20, 24),
            ScrollStrategy::FullRedraw
        ));
    }

    #[test]
    fn cell_style_diff_identical() {
        let a = CellStyle::default();
        let mut buf = String::new();
        a.diff_from(&a, &mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn cell_style_diff_add_bold() {
        let prev = CellStyle::default();
        let next = CellStyle {
            bold: true,
            ..Default::default()
        };
        let mut buf = String::new();
        next.diff_from(&prev, &mut buf);
        assert_eq!(buf, "\x1b[1m");
    }

    #[test]
    fn cell_style_diff_needs_reset() {
        let prev = CellStyle {
            bold: true,
            fg: Some(196),
            ..Default::default()
        };
        let next = CellStyle::default();
        let mut buf = String::new();
        next.diff_from(&prev, &mut buf);
        assert!(buf.starts_with(ansi::RESET));
    }

    #[test]
    fn row_hash_deterministic() {
        let h1 = row_hash(b"hello world");
        let h2 = row_hash(b"hello world");
        let h3 = row_hash(b"hello worle");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn ansi_cursor_to() {
        let mut buf = String::new();
        ansi::cursor_to(&mut buf, 5, 10);
        assert_eq!(buf, "\x1b[5;10H");
    }

    #[test]
    fn glyph_atlas_insert_and_lookup() {
        let mut atlas = GlyphAtlas::new(8, 16, 64);
        let slot1 = atlas.get_or_insert(b'A' as u32, 0, |w, h| vec![0xFFu8; (w * h * 4) as usize]);
        let slot2 = atlas.get_or_insert(b'B' as u32, 0, |w, h| vec![0xAAu8; (w * h * 4) as usize]);
        let slot1_again = atlas.get_or_insert(b'A' as u32, 0, |_, _| panic!("should be cached"));
        assert_eq!(slot1, slot1_again);
        assert_ne!(slot1, slot2);
        assert_eq!(atlas.len(), 2);
    }

    #[test]
    fn glyph_atlas_uv_rect() {
        let atlas = GlyphAtlas::new(8, 16, 4);
        let (u0, v0, u1, v1) = atlas.uv_rect(0);
        assert!((u0 - 0.0).abs() < f32::EPSILON);
        assert!((v0 - 0.0).abs() < f32::EPSILON);
        assert!(u1 > u0);
        assert!(v1 > v0);
    }

    #[test]
    fn damage_rect_overlap() {
        let a = DamageRect::new(0, 5, 0, 10);
        let b = DamageRect::new(3, 8, 5, 15);
        assert!(a.overlaps(&b));
        let c = DamageRect::new(6, 8, 0, 10);
        assert!(!a.overlaps(&c));
    }

    #[test]
    fn damage_rect_to_scissor() {
        let r = DamageRect::new(1, 3, 2, 5);
        let s = r.to_scissor(8.0, 16.0, 0.0, 0.0);
        assert!((s.left - 16.0).abs() < f32::EPSILON);
        assert!((s.top - 16.0).abs() < f32::EPSILON);
        assert!((s.right - 40.0).abs() < f32::EPSILON);
        assert!((s.bottom - 48.0).abs() < f32::EPSILON);
    }

    #[test]
    fn coalesce_damage_rects_merges_adjacent() {
        let rects = vec![
            DamageRect::new(0, 1, 0, 80),
            DamageRect::new(1, 2, 0, 80),
            DamageRect::new(5, 6, 0, 80),
        ];
        let merged = coalesce_damage_rects(&rects, 4);
        assert_eq!(merged.len(), 2); // rows 0-2 merged, row 5 separate
    }

    #[test]
    fn damage_rects_from_rows_merges_consecutive() {
        let rows = vec![1, 2, 3, 7, 8];
        let rects = damage_rects_from_rows(&rows, 80);
        assert_eq!(rects.len(), 2); // rows 1-4, rows 7-9
        assert_eq!(rects[0].row_start, 1);
        assert_eq!(rects[0].row_end, 4);
        assert_eq!(rects[1].row_start, 7);
        assert_eq!(rects[1].row_end, 9);
    }
}
