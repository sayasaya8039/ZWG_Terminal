use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::*;
use parking_lot::Mutex;
use smallvec::SmallVec;
use unicode_width::UnicodeWidthChar;

#[cfg(test)]
use super::{DEFAULT_BG, DEFAULT_FG};

/// Cross-frame glyph layout cache type.
/// Persists shaped glyphs across render frames to avoid redundant text shaping.
/// RwLock: reads vastly outnumber writes — avoids contention on the hot path.
#[cfg(feature = "ghostty_vt")]
pub(super) type GlyphCache = Arc<parking_lot::RwLock<HashMap<GlyphKey, PreparedGlyphPlan>>>;

#[cfg(not(feature = "ghostty_vt"))]
pub(super) type GlyphCache = ();

#[cfg(any(test, not(feature = "ghostty_vt")))]
const BRAILLE_BLANK: char = '\u{2800}';

#[cfg(feature = "ghostty_vt")]
pub(crate) const GHOSTTY_FLAG_BOLD: u8 = 0x02;
#[cfg(feature = "ghostty_vt")]
pub(crate) const GHOSTTY_FLAG_ITALIC: u8 = 0x04;
#[cfg(feature = "ghostty_vt")]
pub(crate) const GHOSTTY_FLAG_UNDERLINE: u8 = 0x08;
#[cfg(feature = "ghostty_vt")]
pub(crate) const GHOSTTY_FLAG_STRIKETHROUGH: u8 = 0x40;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SelectionPoint {
    pub row: u16,
    pub col: u16,
}

#[derive(Clone, Default)]
pub(super) struct CachedTerminalRow {
    pub text: SharedString,
    #[cfg(feature = "ghostty_vt")]
    pub style_runs: Vec<ghostty_vt::StyleRun>,
    #[cfg(feature = "ghostty_vt")]
    pub cells: Vec<GridCell>,
    #[cfg(feature = "ghostty_vt")]
    pub glyph_instances: Vec<GlyphInstance>,
    #[cfg(feature = "ghostty_vt")]
    pub damage_spans: Vec<DamageSpan>,
    #[cfg(feature = "ghostty_vt")]
    pub damaged_glyph_instances: Vec<GlyphInstance>,
}

#[derive(Clone)]
pub(super) struct TerminalSnapshot {
    pub rows: Vec<CachedTerminalRow>,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub cursor_visible: bool,
    pub content_revision: u64,
    pub damaged_rows: Vec<u16>,
    /// Alacritty-inspired 2-frame damage double buffer.
    /// `prev_damaged_rows` holds damage from the previous frame so that
    /// compositors / partial-redraw paths don't miss stale regions.
    pub prev_damaged_rows: Vec<u16>,
}

impl TerminalSnapshot {
    pub fn new(rows: u16) -> Self {
        Self {
            rows: vec![CachedTerminalRow::default(); rows as usize],
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: true,
            content_revision: 0,
            damaged_rows: Vec::new(),
            prev_damaged_rows: Vec::new(),
        }
    }

    pub fn resize(&mut self, rows: u16) {
        self.rows
            .resize(rows as usize, CachedTerminalRow::default());
        self.damaged_rows.clear();
        self.prev_damaged_rows.clear();
    }

    /// Swap damage buffers — call after each frame render.
    /// Previous frame's damage is preserved for 2-frame compositing.
    pub fn swap_damage(&mut self) {
        std::mem::swap(&mut self.prev_damaged_rows, &mut self.damaged_rows);
        self.damaged_rows.clear();
    }

    /// Combined damage from current + previous frame (for 2-frame compositing).
    pub fn combined_damage(&self) -> SmallVec<[u16; 32]> {
        let mut combined: SmallVec<[u16; 32]> = SmallVec::new();
        combined.extend_from_slice(&self.damaged_rows);
        for &row in &self.prev_damaged_rows {
            if !combined.contains(&row) {
                combined.push(row);
            }
        }
        combined
    }
}

pub(super) struct GridRendererConfig {
    pub cell_width: f32,
    pub cell_height: f32,
    pub font_size: f32,
    pub horizontal_text_padding: f32,
    pub term_cols: u16,
    pub fg_color: u32,
    pub bg_color: u32,
    /// Pre-built Font descriptor — avoids rebuilding from font family string every frame.
    pub cached_font: Font,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GridCellKind {
    Text,
    GeometricBlock,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GridCell {
    pub col: u16,
    pub width: u8,
    pub glyph: String,
    pub fg_rgb: u32,
    pub bg_rgb: u32,
    pub flags: u8,
    pub kind: GridCellKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum GlyphRenderPath {
    AtlasMonochrome,
    AtlasPolychrome,
    Geometry,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct GlyphKey {
    pub glyph: String,
    pub width: u8,
    pub flags: u8,
    pub render_path: GlyphRenderPath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlyphInstance {
    pub row: u16,
    pub col: u16,
    pub fg_rgb: u32,
    pub bg_rgb: u32,
    pub key: GlyphKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DamageSpan {
    pub start_col: u16,
    pub end_col: u16,
}

#[cfg(feature = "ghostty_vt")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PreparedGlyph {
    pub font_id: FontId,
    pub glyph_id: GlyphId,
    pub position: Point<Pixels>,
    pub is_emoji: bool,
}

#[cfg(feature = "ghostty_vt")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PreparedGlyphPlan {
    pub font_size: Pixels,
    pub baseline_y: Pixels,
    pub glyphs: Vec<PreparedGlyph>,
}

#[cfg(feature = "ghostty_vt")]
impl Default for PreparedGlyphPlan {
    fn default() -> Self {
        Self {
            font_size: px(0.0),
            baseline_y: px(0.0),
            glyphs: Vec::new(),
        }
    }
}

pub(super) fn char_cell_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

pub(super) fn col_to_char_index(text: &str, target_col: usize) -> usize {
    let mut col = 0;
    for (i, ch) in text.chars().enumerate() {
        if col >= target_col {
            return i;
        }
        col += char_cell_width(ch);
    }
    text.chars().count()
}

pub(crate) fn is_geometric_block_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{2580}'
            | '\u{2584}'
            | '\u{2588}'
            | '\u{258C}'
            | '\u{2590}'
            | '\u{2591}'
            | '\u{2592}'
            | '\u{2593}'
    )
}

pub(crate) fn is_box_drawing_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{2500}'
            | '\u{2501}'
            | '\u{2502}'
            | '\u{2503}'
            | '\u{250C}'
            | '\u{2510}'
            | '\u{2514}'
            | '\u{2518}'
            | '\u{251C}'
            | '\u{2524}'
            | '\u{252C}'
            | '\u{2534}'
            | '\u{253C}'
            | '\u{250F}'
            | '\u{2513}'
            | '\u{2517}'
            | '\u{251B}'
            | '\u{2523}'
            | '\u{252B}'
            | '\u{2533}'
            | '\u{253B}'
            | '\u{254B}'
            | '\u{2550}'
            | '\u{2551}'
            | '\u{2554}'
            | '\u{2557}'
            | '\u{255A}'
            | '\u{255D}'
            | '\u{2560}'
            | '\u{2563}'
            | '\u{2566}'
            | '\u{2569}'
            | '\u{256C}'
            | '\u{256D}'
            | '\u{256E}'
            | '\u{2570}'
            | '\u{256F}'
    )
}

pub(crate) fn is_geometry_char(ch: char) -> bool {
    is_geometric_block_char(ch) || is_box_drawing_char(ch)
}

#[cfg(any(test, not(feature = "ghostty_vt")))]
pub(crate) fn sanitize_text_for_shaping(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if is_geometric_block_char(ch) {
                BRAILLE_BLANK
            } else {
                ch
            }
        })
        .collect()
}

fn looks_like_emoji(glyph: &str) -> bool {
    glyph.chars().any(|ch| {
        matches!(
            ch,
            '\u{1F300}'..='\u{1FAFF}'
                | '\u{2600}'..='\u{27BF}'
                | '\u{FE0F}'
        )
    })
}

pub(crate) fn glyph_requires_gpui_overlay(glyph: &str) -> bool {
    if glyph.is_empty() {
        return false;
    }
    if looks_like_emoji(glyph) {
        return true;
    }

    let mut chars = glyph.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => ch as u32 > 0xFFFF,
        (_, Some(_)) => true,
        _ => false,
    }
}

pub(crate) fn glyph_key_from_cell(cell: &GridCell) -> GlyphKey {
    let render_path = match cell.kind {
        GridCellKind::GeometricBlock => GlyphRenderPath::Geometry,
        GridCellKind::Text if looks_like_emoji(&cell.glyph) => GlyphRenderPath::AtlasPolychrome,
        GridCellKind::Text => GlyphRenderPath::AtlasMonochrome,
    };

    GlyphKey {
        glyph: cell.glyph.clone(),
        width: cell.width,
        flags: cell.flags,
        render_path,
    }
}

pub(crate) fn glyph_instances_from_cells(cells: &[GridCell], row: u16) -> Vec<GlyphInstance> {
    let mut instances = Vec::with_capacity(cells.len());
    for cell in cells {
        instances.push(GlyphInstance {
            row,
            col: cell.col,
            fg_rgb: cell.fg_rgb,
            bg_rgb: cell.bg_rgb,
            key: glyph_key_from_cell(cell),
        });
    }
    instances
}

#[cfg(feature = "ghostty_vt")]
fn spans_overlap(start: u16, end: u16, span: DamageSpan) -> bool {
    start < span.end_col && span.start_col < end
}

#[cfg(feature = "ghostty_vt")]
fn cell_overlaps_damage(cell: &GridCell, spans: &[DamageSpan]) -> bool {
    let start = cell.col;
    let end = cell.col.saturating_add(cell.width as u16);
    // SIMD fast path: 8スパン以上の場合 AVX2 で一括判定
    #[cfg(target_arch = "x86_64")]
    {
        if spans.len() >= 8 && is_x86_feature_detected!("avx2") {
            return unsafe { simd_check_damage_overlap(spans, start, end) };
        }
    }
    spans
        .iter()
        .copied()
        .any(|span| spans_overlap(start, end, span))
}

#[cfg(feature = "ghostty_vt")]
fn instance_overlaps_damage(instance: &GlyphInstance, spans: &[DamageSpan]) -> bool {
    let start = instance.col;
    let end = instance.col.saturating_add(instance.key.width as u16);
    // SIMD fast path: 8スパン以上の場合 AVX2 で一括判定
    #[cfg(target_arch = "x86_64")]
    {
        if spans.len() >= 8 && is_x86_feature_detected!("avx2") {
            return unsafe { simd_check_damage_overlap(spans, start, end) };
        }
    }
    spans
        .iter()
        .copied()
        .any(|span| spans_overlap(start, end, span))
}

/// SIMD (AVX2) によるダメージスパン重複一括判定
/// 8個の DamageSpan を 128bit (start_col × 8 = 128bit) ずつロードし、
/// 比較を並列実行する。
#[cfg(all(feature = "ghostty_vt", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn simd_check_damage_overlap(spans: &[DamageSpan], query_start: u16, query_end: u16) -> bool {
    use std::arch::x86_64::*;

    // DamageSpan は (start_col: u16, end_col: u16) = 4バイト
    // SSE2 で 8スパン = 32バイトを一括処理
    let len = spans.len();
    let mut i = 0;

    let q_start = _mm256_set1_epi16(query_start as i16);
    let q_end = _mm256_set1_epi16(query_end as i16);

    // 16個のu16 = 8 DamageSpan (start, end が交互)
    while i + 8 <= len {
        // DamageSpan の配列を u16 としてロード
        // メモリレイアウト: [s0, e0, s1, e1, s2, e2, s3, e3, s4, e4, s5, e5, s6, e6, s7, e7]
        let raw_ptr = spans.as_ptr().add(i) as *const __m256i;
        let raw = _mm256_loadu_si256(raw_ptr);

        // start_col を偶数位置、end_col を奇数位置から抽出
        // シャッフルで start と end を分離
        // 偶数インデックス (0,2,4,6,8,10,12,14) → starts
        // 奇数インデックス (1,3,5,7,9,11,13,15) → ends
        let starts = _mm256_and_si256(raw, _mm256_set1_epi32(0x0000FFFF));
        let ends = _mm256_srli_epi32(raw, 16);

        // 16bit として比較するため pack
        let starts_16 = _mm256_packs_epi32(starts, _mm256_setzero_si256());
        let ends_16 = _mm256_packs_epi32(ends, _mm256_setzero_si256());

        // 重複条件: query_start < end AND start < query_end
        // _mm256_cmpgt_epi16 は signed 比較なので u16 範囲内は安全
        let cond1 = _mm256_cmpgt_epi16(ends_16, q_start);   // end > query_start
        let cond2 = _mm256_cmpgt_epi16(q_end, starts_16);   // query_end > start
        let overlap = _mm256_and_si256(cond1, cond2);

        if _mm256_movemask_epi8(overlap) != 0 {
            return true;
        }

        i += 8;
    }

    // 残りスパンのスカラー処理
    while i < len {
        if spans_overlap(query_start, query_end, spans[i]) {
            return true;
        }
        i += 1;
    }

    false
}

#[cfg(feature = "ghostty_vt")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct ColumnDamageSignature {
    head_col: u16,
    width: u8,
    glyph: String,
    fg_rgb: u32,
    bg_rgb: u32,
    flags: u8,
    kind: GridCellKind,
}

#[cfg(feature = "ghostty_vt")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StyleDamageSignature {
    fg_rgb: u32,
    bg_rgb: u32,
    flags: u8,
}

#[cfg(feature = "ghostty_vt")]
fn column_damage_signatures(
    cells: &[GridCell],
    max_cols: u16,
) -> SmallVec<[Option<ColumnDamageSignature>; 256]> {
    let mut columns: SmallVec<[Option<ColumnDamageSignature>; 256]> =
        smallvec::smallvec![None; max_cols as usize];
    for cell in cells {
        let signature = ColumnDamageSignature {
            head_col: cell.col,
            width: cell.width,
            glyph: cell.glyph.clone(),
            fg_rgb: cell.fg_rgb,
            bg_rgb: cell.bg_rgb,
            flags: cell.flags,
            kind: cell.kind,
        };

        let start = cell.col.min(max_cols);
        let end = cell.col.saturating_add(cell.width as u16).min(max_cols);
        for col in start..end {
            columns[col as usize] = Some(signature.clone());
        }
    }

    columns
}

#[cfg(feature = "ghostty_vt")]
fn style_damage_signatures(
    style_runs: &[ghostty_vt::StyleRun],
    term_cols: u16,
    default_fg: u32,
    default_bg: u32,
) -> SmallVec<[StyleDamageSignature; 256]> {
    let mut sigs = SmallVec::with_capacity(term_cols as usize);
    for col in 0..term_cols {
        let (fg_rgb, bg_rgb, flags) =
            grid_cell_style_at(style_runs, col + 1, default_fg, default_bg);
        sigs.push(StyleDamageSignature {
            fg_rgb,
            bg_rgb,
            flags,
        });
    }
    sigs
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn merge_damage_spans(mut spans: Vec<DamageSpan>) -> Vec<DamageSpan> {
    if spans.is_empty() {
        return spans;
    }

    spans.sort_unstable_by_key(|span| (span.start_col, span.end_col));
    let mut merged = vec![spans[0]];
    for span in spans.into_iter().skip(1) {
        let last = merged.last_mut().expect("merged spans always has a head");
        if span.start_col <= last.end_col {
            last.end_col = last.end_col.max(span.end_col);
        } else {
            merged.push(span);
        }
    }

    merged
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn full_row_damage(term_cols: u16) -> Vec<DamageSpan> {
    merge_damage_spans(if term_cols == 0 {
        Vec::new()
    } else {
        vec![DamageSpan {
            start_col: 0,
            end_col: term_cols,
        }]
    })
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn damage_spans_from_cells(
    previous: &[GridCell],
    next: &[GridCell],
    term_cols: u16,
) -> Vec<DamageSpan> {
    let previous_columns = column_damage_signatures(previous, term_cols);
    let next_columns = column_damage_signatures(next, term_cols);
    let mut spans = Vec::with_capacity((term_cols as usize / 8).max(4));
    let mut span_start = None;

    for col in 0..term_cols {
        let changed = previous_columns[col as usize] != next_columns[col as usize];
        match (span_start, changed) {
            (None, true) => span_start = Some(col),
            (Some(start_col), false) => {
                spans.push(DamageSpan {
                    start_col,
                    end_col: col,
                });
                span_start = None;
            }
            _ => {}
        }
    }

    if let Some(start_col) = span_start {
        spans.push(DamageSpan {
            start_col,
            end_col: term_cols,
        });
    }

    spans
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn damage_spans_from_style_runs(
    previous: &[ghostty_vt::StyleRun],
    next: &[ghostty_vt::StyleRun],
    term_cols: u16,
    default_fg: u32,
    default_bg: u32,
) -> Vec<DamageSpan> {
    let previous_columns = style_damage_signatures(previous, term_cols, default_fg, default_bg);
    let next_columns = style_damage_signatures(next, term_cols, default_fg, default_bg);
    let mut spans = Vec::with_capacity((term_cols as usize / 8).max(4));
    let mut span_start = None;

    for col in 0..term_cols {
        let changed = previous_columns[col as usize] != next_columns[col as usize];
        match (span_start, changed) {
            (None, true) => span_start = Some(col),
            (Some(start_col), false) => {
                spans.push(DamageSpan {
                    start_col,
                    end_col: col,
                });
                span_start = None;
            }
            _ => {}
        }
    }

    if let Some(start_col) = span_start {
        spans.push(DamageSpan {
            start_col,
            end_col: term_cols,
        });
    }

    spans
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn damage_spans_from_terminal_row(
    previous_cells: &[GridCell],
    previous_styles: &[ghostty_vt::StyleRun],
    next_cells: &[GridCell],
    next_styles: &[ghostty_vt::StyleRun],
    term_cols: u16,
    default_fg: u32,
    default_bg: u32,
) -> Vec<DamageSpan> {
    // Fast path: if both cells and styles are identical references or equal, skip scan.
    if previous_cells == next_cells && previous_styles == next_styles {
        return Vec::new();
    }
    merge_damage_spans(
        damage_spans_from_cells(previous_cells, next_cells, term_cols)
            .into_iter()
            .chain(damage_spans_from_style_runs(
                previous_styles,
                next_styles,
                term_cols,
                default_fg,
                default_bg,
            ))
            .collect(),
    )
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn patch_cells_in_damage(
    previous: &[GridCell],
    next: &[GridCell],
    damage_spans: &[DamageSpan],
) -> Vec<GridCell> {
    if damage_spans.is_empty() {
        return previous.to_vec();
    }

    let mut cells = Vec::with_capacity(previous.len().max(next.len()));
    for cell in previous {
        if !cell_overlaps_damage(cell, damage_spans) {
            cells.push(cell.clone());
        }
    }
    for cell in next {
        if cell_overlaps_damage(cell, damage_spans) {
            cells.push(cell.clone());
        }
    }
    cells.sort_unstable_by_key(|cell| cell.col);
    cells
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn glyph_instances_in_damage(
    cells: &[GridCell],
    row: u16,
    damage_spans: &[DamageSpan],
) -> Vec<GlyphInstance> {
    if damage_spans.is_empty() {
        return Vec::new();
    }

    glyph_instances_from_cells(cells, row)
        .into_iter()
        .filter(|instance| instance_overlaps_damage(instance, damage_spans))
        .collect()
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn patch_glyph_instances_in_damage(
    previous: &[GlyphInstance],
    next_cells: &[GridCell],
    row: u16,
    damage_spans: &[DamageSpan],
) -> Vec<GlyphInstance> {
    if damage_spans.is_empty() {
        return previous.to_vec();
    }

    let mut instances = Vec::with_capacity(previous.len());
    for inst in previous {
        if !instance_overlaps_damage(inst, damage_spans) {
            instances.push(inst.clone());
        }
    }
    instances.extend(glyph_instances_in_damage(next_cells, row, damage_spans));
    instances.sort_unstable_by_key(|instance| instance.col);
    instances
}

#[cfg(all(test, feature = "ghostty_vt"))]
pub(crate) fn glyph_instances_from_row(
    row: &CachedTerminalRow,
    row_idx: u16,
) -> Vec<GlyphInstance> {
    glyph_instances_from_cells(&row.cells, row_idx)
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn collect_active_glyph_keys(snapshot: &TerminalSnapshot) -> HashSet<GlyphKey> {
    snapshot
        .rows
        .iter()
        .flat_map(|row| row.glyph_instances.iter())
        .filter(|instance| instance.key.render_path != GlyphRenderPath::Geometry)
        .map(|instance| instance.key.clone())
        .collect()
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn collect_damaged_glyph_keys(snapshot: &TerminalSnapshot) -> HashSet<GlyphKey> {
    snapshot
        .rows
        .iter()
        .flat_map(|row| row.damaged_glyph_instances.iter())
        .filter(|instance| instance.key.render_path != GlyphRenderPath::Geometry)
        .map(|instance| instance.key.clone())
        .collect()
}

#[cfg(feature = "ghostty_vt")]
fn borrow_active_glyph_plans(
    cache: &HashMap<GlyphKey, PreparedGlyphPlan>,
    active_keys: &HashSet<GlyphKey>,
) -> HashMap<GlyphKey, PreparedGlyphPlan> {
    let mut result = HashMap::with_capacity(active_keys.len());
    for key in active_keys {
        if let Some(plan) = cache.get(key) {
            result.insert(key.clone(), plan.clone());
        }
    }
    result
}

#[cfg(feature = "ghostty_vt")]
fn persist_active_glyph_plans(
    cache: &mut HashMap<GlyphKey, PreparedGlyphPlan>,
    local_plans: HashMap<GlyphKey, PreparedGlyphPlan>,
    active_keys: &HashSet<GlyphKey>,
) {
    // Move plans from local into cache — avoids cloning keys and plans
    for (key, plan) in local_plans {
        if active_keys.contains(&key) {
            cache.insert(key, plan);
        }
    }
}

/// Maximum glyph cache size — prevents unbounded growth while keeping
/// recently used glyphs warm.  Inspired by Alacritty's glyph cache which
/// never evicts (relying on full reset), but we add a soft ceiling to
/// avoid memory bloat for long-running sessions with many unique glyphs.
#[cfg(feature = "ghostty_vt")]
const GLYPH_CACHE_SOFT_MAX: usize = 4096;

#[cfg(feature = "ghostty_vt")]
fn prune_glyph_cache(
    cache: &mut HashMap<GlyphKey, PreparedGlyphPlan>,
    active_keys: &HashSet<GlyphKey>,
) {
    // Only prune if cache exceeds soft max — avoids thrashing on normal usage.
    // Alacritty never prunes (grows unbounded), but we add a ceiling.
    if cache.len() > GLYPH_CACHE_SOFT_MAX {
        cache.retain(|key, _| active_keys.contains(key));
    }
}

/// Pre-warm the glyph cache with ASCII printable characters (32-126).
/// Inspired by Alacritty's `load_common_glyphs()` which preloads ASCII
/// across all font variants to eliminate first-frame shaping latency.
#[cfg(feature = "ghostty_vt")]
pub(crate) fn preload_ascii_glyphs(
    text_system: &WindowTextSystem,
    font_desc: &Font,
    font_size: Pixels,
    cell_height: f32,
    cache: &mut HashMap<GlyphKey, PreparedGlyphPlan>,
) {
    // ASCII printable range + common box-drawing characters
    let chars: Vec<char> = (32u8..=126)
        .map(|b| b as char)
        .chain(['│', '─', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '█', '▀', '▄'].into_iter())
        .collect();

    let flag_variants: &[u8] = &[
        0,                      // regular
        GHOSTTY_FLAG_BOLD,      // bold
        GHOSTTY_FLAG_ITALIC,    // italic
        GHOSTTY_FLAG_BOLD | GHOSTTY_FLAG_ITALIC, // bold+italic
    ];

    for &ch in &chars {
        let glyph_str = ch.to_string();
        let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1) as u8;
        for &flags in flag_variants {
            let key = GlyphKey {
                glyph: glyph_str.clone(),
                width,
                flags,
                render_path: GlyphRenderPath::AtlasMonochrome,
            };
            // Use or_insert_with to avoid reshaping if already cached
            let _ = glyph_layout_for_key(&key, text_system, font_desc, font_size, cell_height, cache);
        }
    }
    log::info!(
        "ASCII glyph preload complete: {} entries in cache ({}×{} variants)",
        cache.len(),
        chars.len(),
        flag_variants.len()
    );
}

// ── RAMdisk glyph key cache ─────────────────────────────────────────

const GLYPH_KEYS_CACHE_FILE: &str = "glyph_keys.json";

/// Save the current glyph cache keys to RAMdisk for fast startup next time.
/// Only GlyphKey is saved (not PreparedGlyphPlan) because PreparedGlyphPlan
/// contains GPUI types (FontId, GlyphId, Pixels) that are non-serializable
/// and session-specific. The keys serve as a "prewarm list" on next boot.
#[cfg(feature = "ghostty_vt")]
pub(crate) fn save_glyph_keys_to_ramdisk(cache: &HashMap<GlyphKey, PreparedGlyphPlan>) {
    let cache_dir = match std::env::var("ZWG_RAMDISK_CACHE") {
        Ok(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
        _ => return, // No RAMdisk — silently skip
    };

    let keys: Vec<&GlyphKey> = cache.keys().collect();
    if keys.is_empty() {
        return;
    }

    let path = cache_dir.join(GLYPH_KEYS_CACHE_FILE);
    match serde_json::to_vec(&keys) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, &data) {
                log::warn!("Failed to save glyph keys to RAMdisk: {e}");
            } else {
                log::info!(
                    "Saved {} glyph keys to RAMdisk ({})",
                    keys.len(),
                    path.display()
                );
            }
        }
        Err(e) => log::warn!("Failed to serialize glyph keys: {e}"),
    }
}

/// Load glyph keys from RAMdisk. Returns None if RAMdisk is unavailable
/// or no cached keys exist yet.
#[cfg(feature = "ghostty_vt")]
pub(crate) fn load_glyph_keys_from_ramdisk() -> Option<Vec<GlyphKey>> {
    let cache_dir = std::env::var("ZWG_RAMDISK_CACHE").ok()?;
    if cache_dir.is_empty() {
        return None;
    }

    let path = std::path::PathBuf::from(&cache_dir).join(GLYPH_KEYS_CACHE_FILE);
    let data = std::fs::read(&path).ok()?;
    match serde_json::from_slice::<Vec<GlyphKey>>(&data) {
        Ok(keys) => {
            log::info!(
                "Loaded {} cached glyph keys from RAMdisk ({})",
                keys.len(),
                path.display()
            );
            Some(keys)
        }
        Err(e) => {
            log::warn!("Failed to deserialize glyph keys from RAMdisk: {e}");
            None
        }
    }
}

/// Prewarm cache with previously-seen glyph keys loaded from RAMdisk.
/// Called after preload_ascii_glyphs() to cover non-ASCII characters
/// (CJK, box-drawing, emoji, etc.) that were used in previous sessions.
#[cfg(feature = "ghostty_vt")]
pub(crate) fn prewarm_from_ramdisk_keys(
    text_system: &WindowTextSystem,
    font_desc: &Font,
    font_size: Pixels,
    cell_height: f32,
    cache: &mut HashMap<GlyphKey, PreparedGlyphPlan>,
) {
    let keys = match load_glyph_keys_from_ramdisk() {
        Some(k) => k,
        None => return,
    };

    let before = cache.len();
    for key in &keys {
        if !cache.contains_key(key) {
            let _ = glyph_layout_for_key(key, text_system, font_desc, font_size, cell_height, cache);
        }
    }
    let added = cache.len() - before;
    if added > 0 {
        log::info!(
            "RAMdisk prewarm: shaped {} new glyphs ({} total in cache)",
            added,
            cache.len()
        );
    }
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn resolve_cursor_cell(
    cursor_col: u16,
    row_cells: &[GridCell],
    term_cols: u16,
) -> (u16, u8) {
    let clamped = cursor_col.min(term_cols.saturating_sub(1));
    for cell in row_cells {
        let start = cell.col;
        let end = cell.col.saturating_add(cell.width as u16);
        if start <= clamped && clamped < end {
            return (start, cell.width);
        }
    }

    (clamped, 1)
}

#[cfg(feature = "ghostty_vt")]
fn text_run_for_glyph_key(key: &GlyphKey, font_desc: &Font, color: Hsla) -> TextRun {
    let mut run_font = font_desc.clone();
    if key.flags & GHOSTTY_FLAG_BOLD != 0 {
        run_font.weight = FontWeight::BOLD;
    }
    if key.flags & GHOSTTY_FLAG_ITALIC != 0 {
        run_font.style = FontStyle::Italic;
    }

    TextRun {
        len: key.glyph.len(),
        font: run_font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

#[cfg(feature = "ghostty_vt")]
fn glyph_layout_for_key(
    key: &GlyphKey,
    text_system: &WindowTextSystem,
    font_desc: &Font,
    font_size: Pixels,
    cell_height: f32,
    cache: &mut HashMap<GlyphKey, PreparedGlyphPlan>,
) -> PreparedGlyphPlan {
    cache
        .entry(key.clone())
        .or_insert_with(|| {
            let run = text_run_for_glyph_key(key, font_desc, Hsla::default());
            let shaped = text_system.shape_line(
                SharedString::from(key.glyph.clone()),
                font_size,
                &[run],
                None,
            );
            glyph_plan_from_layout(&shaped, cell_height)
        })
        .clone()
}

#[cfg(feature = "ghostty_vt")]
fn glyph_baseline_y(layout: &LineLayout, cell_height: f32) -> Pixels {
    let padding_top = (px(cell_height) - layout.ascent - layout.descent) / 2.;
    padding_top + layout.ascent
}

#[cfg(feature = "ghostty_vt")]
fn glyph_plan_from_layout(layout: &LineLayout, cell_height: f32) -> PreparedGlyphPlan {
    let glyphs = layout
        .runs
        .iter()
        .flat_map(|run| {
            run.glyphs.iter().map(move |glyph| PreparedGlyph {
                font_id: run.font_id,
                glyph_id: glyph.id,
                position: glyph.position,
                is_emoji: glyph.is_emoji,
            })
        })
        .collect();

    PreparedGlyphPlan {
        font_size: layout.font_size,
        baseline_y: glyph_baseline_y(layout, cell_height),
        glyphs,
    }
}

#[cfg(feature = "ghostty_vt")]
fn paint_glyph_instance(
    instance: &GlyphInstance,
    plan: &PreparedGlyphPlan,
    bounds: Bounds<Pixels>,
    config: &GridRendererConfig,
    window: &mut Window,
) {
    let glyph_color = Hsla::from(rgb(instance.fg_rgb));
    let cell_origin = point(
        bounds.origin.x
            + px(config.horizontal_text_padding + instance.col as f32 * config.cell_width),
        bounds.origin.y + px(instance.row as f32 * config.cell_height),
    );
    let baseline_origin = point(cell_origin.x, cell_origin.y + plan.baseline_y);

    for glyph in &plan.glyphs {
        let glyph_origin = point(
            baseline_origin.x + glyph.position.x,
            baseline_origin.y + glyph.position.y,
        );
        if glyph.is_emoji {
            let _ = window.paint_emoji(glyph_origin, glyph.font_id, glyph.glyph_id, plan.font_size);
        } else {
            let _ = window.paint_glyph(
                glyph_origin,
                glyph.font_id,
                glyph.glyph_id,
                plan.font_size,
                glyph_color,
            );
        }
    }

    let deco_width = px(config.cell_width * instance.key.width as f32);
    if instance.key.flags & GHOSTTY_FLAG_UNDERLINE != 0 {
        let y = cell_origin.y + px(config.cell_height - 2.0);
        window.paint_quad(fill(
            Bounds::new(point(cell_origin.x, y), size(deco_width, px(1.0))),
            glyph_color,
        ));
    }
    if instance.key.flags & GHOSTTY_FLAG_STRIKETHROUGH != 0 {
        let y = cell_origin.y + px(config.cell_height * 0.5);
        window.paint_quad(fill(
            Bounds::new(point(cell_origin.x, y), size(deco_width, px(1.0))),
            glyph_color,
        ));
    }
}

fn paint_geometry_rect(
    window: &mut Window,
    origin: Point<Pixels>,
    width: f32,
    height: f32,
    color: Hsla,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    window.paint_quad(fill(
        Bounds::new(origin, size(px(width), px(height))),
        color,
    ));
}

pub(crate) fn paint_geometry_cell(
    row: u16,
    cell: &GridCell,
    bounds: Bounds<Pixels>,
    config: &GridRendererConfig,
    window: &mut Window,
) {
    let fg = Hsla::from(rgb(cell.fg_rgb));
    let x =
        bounds.origin.x + px(config.horizontal_text_padding + cell.col as f32 * config.cell_width);
    let y = bounds.origin.y + px(row as f32 * config.cell_height);
    let cell_width = config.cell_width * cell.width as f32;
    let cell_height = config.cell_height;
    let mid_x = cell_width * 0.5;
    let mid_y = cell_height * 0.5;
    let light = (cell_width.min(cell_height) * 0.12).max(1.0).ceil();
    let heavy = (light * 1.8).max(2.0).ceil();
    let edge_overdraw = 0.5;
    let horizontal = |window: &mut Window, start: f32, end: f32, thickness: f32| {
        let start = if start <= 0.0 {
            -edge_overdraw
        } else {
            start - edge_overdraw
        };
        let end = if end >= cell_width {
            cell_width + edge_overdraw
        } else {
            end + edge_overdraw
        };
        paint_geometry_rect(
            window,
            point(x + px(start), y + px((cell_height - thickness) * 0.5)),
            end - start,
            thickness,
            fg,
        );
    };
    let vertical = |window: &mut Window, start: f32, end: f32, thickness: f32| {
        let start = if start <= 0.0 {
            -edge_overdraw
        } else {
            start - edge_overdraw
        };
        let end = if end >= cell_height {
            cell_height + edge_overdraw
        } else {
            end + edge_overdraw
        };
        paint_geometry_rect(
            window,
            point(x + px((cell_width - thickness) * 0.5), y + px(start)),
            thickness,
            end - start,
            fg,
        );
    };
    let double_horizontal = |window: &mut Window, start: f32, end: f32| {
        let lane = (cell_height * 0.24).max(1.0);
        let start = if start <= 0.0 {
            -edge_overdraw
        } else {
            start - edge_overdraw
        };
        let end = if end >= cell_width {
            cell_width + edge_overdraw
        } else {
            end + edge_overdraw
        };
        paint_geometry_rect(
            window,
            point(x + px(start), y + px(lane - light * 0.5)),
            end - start,
            light,
            fg,
        );
        paint_geometry_rect(
            window,
            point(x + px(start), y + px(cell_height - lane - light * 0.5)),
            end - start,
            light,
            fg,
        );
    };
    let double_vertical = |window: &mut Window, start: f32, end: f32| {
        let lane = (cell_width * 0.24).max(1.0);
        let start = if start <= 0.0 {
            -edge_overdraw
        } else {
            start - edge_overdraw
        };
        let end = if end >= cell_height {
            cell_height + edge_overdraw
        } else {
            end + edge_overdraw
        };
        paint_geometry_rect(
            window,
            point(x + px(lane - light * 0.5), y + px(start)),
            light,
            end - start,
            fg,
        );
        paint_geometry_rect(
            window,
            point(x + px(cell_width - lane - light * 0.5), y + px(start)),
            light,
            end - start,
            fg,
        );
    };
    let ch = cell.glyph.chars().next().unwrap_or(' ');

    match ch {
        '\u{2588}' => paint_geometry_rect(window, point(x, y), cell_width, cell_height, fg),
        '\u{2580}' => paint_geometry_rect(window, point(x, y), cell_width, cell_height * 0.5, fg),
        '\u{2584}' => paint_geometry_rect(
            window,
            point(x, y + px(cell_height * 0.5)),
            cell_width,
            cell_height * 0.5,
            fg,
        ),
        '\u{258C}' => paint_geometry_rect(window, point(x, y), cell_width * 0.5, cell_height, fg),
        '\u{2590}' => paint_geometry_rect(
            window,
            point(x + px(cell_width * 0.5), y),
            cell_width * 0.5,
            cell_height,
            fg,
        ),
        '\u{2591}' => paint_geometry_rect(
            window,
            point(x, y),
            cell_width,
            cell_height,
            fg.opacity(0.25),
        ),
        '\u{2592}' => paint_geometry_rect(
            window,
            point(x, y),
            cell_width,
            cell_height,
            fg.opacity(0.5),
        ),
        '\u{2593}' => paint_geometry_rect(
            window,
            point(x, y),
            cell_width,
            cell_height,
            fg.opacity(0.75),
        ),
        '\u{2500}' => horizontal(window, 0.0, cell_width, light),
        '\u{2501}' => horizontal(window, 0.0, cell_width, heavy),
        '\u{2502}' => vertical(window, 0.0, cell_height, light),
        '\u{2503}' => vertical(window, 0.0, cell_height, heavy),
        '\u{2550}' => double_horizontal(window, 0.0, cell_width),
        '\u{2551}' => double_vertical(window, 0.0, cell_height),
        '\u{250C}' | '\u{256D}' => {
            horizontal(window, mid_x, cell_width, light);
            vertical(window, mid_y, cell_height, light);
        }
        '\u{2510}' | '\u{256E}' => {
            horizontal(window, 0.0, mid_x, light);
            vertical(window, mid_y, cell_height, light);
        }
        '\u{2514}' | '\u{2570}' => {
            horizontal(window, mid_x, cell_width, light);
            vertical(window, 0.0, mid_y, light);
        }
        '\u{2518}' | '\u{256F}' => {
            horizontal(window, 0.0, mid_x, light);
            vertical(window, 0.0, mid_y, light);
        }
        '\u{251C}' => {
            horizontal(window, mid_x, cell_width, light);
            vertical(window, 0.0, cell_height, light);
        }
        '\u{2524}' => {
            horizontal(window, 0.0, mid_x, light);
            vertical(window, 0.0, cell_height, light);
        }
        '\u{252C}' => {
            horizontal(window, 0.0, cell_width, light);
            vertical(window, mid_y, cell_height, light);
        }
        '\u{2534}' => {
            horizontal(window, 0.0, cell_width, light);
            vertical(window, 0.0, mid_y, light);
        }
        '\u{253C}' => {
            horizontal(window, 0.0, cell_width, light);
            vertical(window, 0.0, cell_height, light);
        }
        '\u{250F}' => {
            horizontal(window, mid_x, cell_width, heavy);
            vertical(window, mid_y, cell_height, heavy);
        }
        '\u{2513}' => {
            horizontal(window, 0.0, mid_x, heavy);
            vertical(window, mid_y, cell_height, heavy);
        }
        '\u{2517}' => {
            horizontal(window, mid_x, cell_width, heavy);
            vertical(window, 0.0, mid_y, heavy);
        }
        '\u{251B}' => {
            horizontal(window, 0.0, mid_x, heavy);
            vertical(window, 0.0, mid_y, heavy);
        }
        '\u{2523}' => {
            horizontal(window, mid_x, cell_width, heavy);
            vertical(window, 0.0, cell_height, heavy);
        }
        '\u{252B}' => {
            horizontal(window, 0.0, mid_x, heavy);
            vertical(window, 0.0, cell_height, heavy);
        }
        '\u{2533}' => {
            horizontal(window, 0.0, cell_width, heavy);
            vertical(window, mid_y, cell_height, heavy);
        }
        '\u{253B}' => {
            horizontal(window, 0.0, cell_width, heavy);
            vertical(window, 0.0, mid_y, heavy);
        }
        '\u{254B}' => {
            horizontal(window, 0.0, cell_width, heavy);
            vertical(window, 0.0, cell_height, heavy);
        }
        '\u{2554}' => {
            double_horizontal(window, mid_x, cell_width);
            double_vertical(window, mid_y, cell_height);
        }
        '\u{2557}' => {
            double_horizontal(window, 0.0, mid_x);
            double_vertical(window, mid_y, cell_height);
        }
        '\u{255A}' => {
            double_horizontal(window, mid_x, cell_width);
            double_vertical(window, 0.0, mid_y);
        }
        '\u{255D}' => {
            double_horizontal(window, 0.0, mid_x);
            double_vertical(window, 0.0, mid_y);
        }
        '\u{2560}' => {
            double_horizontal(window, mid_x, cell_width);
            double_vertical(window, 0.0, cell_height);
        }
        '\u{2563}' => {
            double_horizontal(window, 0.0, mid_x);
            double_vertical(window, 0.0, cell_height);
        }
        '\u{2566}' => {
            double_horizontal(window, 0.0, cell_width);
            double_vertical(window, mid_y, cell_height);
        }
        '\u{2569}' => {
            double_horizontal(window, 0.0, cell_width);
            double_vertical(window, 0.0, mid_y);
        }
        '\u{256C}' => {
            double_horizontal(window, 0.0, cell_width);
            double_vertical(window, 0.0, cell_height);
        }
        _ => {}
    }
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn default_grid_cell_style(default_fg: u32, default_bg: u32) -> (u32, u32, u8) {
    (default_fg, default_bg, 0)
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn grid_cell_style_at(
    style_runs: &[ghostty_vt::StyleRun],
    col: u16,
    default_fg: u32,
    default_bg: u32,
) -> (u32, u32, u8) {
    style_runs
        .iter()
        .find(|run| run.start_col <= col && col <= run.end_col)
        .map(|run| {
            let fg = ((run.fg.r as u32) << 16) | ((run.fg.g as u32) << 8) | (run.fg.b as u32);
            let bg = ((run.bg.r as u32) << 16) | ((run.bg.g as u32) << 8) | (run.bg.b as u32);
            (fg, bg, run.flags)
        })
        .unwrap_or_else(|| default_grid_cell_style(default_fg, default_bg))
}

#[cfg(all(test, feature = "ghostty_vt"))]
pub(crate) fn grid_cells_from_row(
    row: &CachedTerminalRow,
    max_cols: u16,
    default_fg: u32,
    default_bg: u32,
) -> Vec<GridCell> {
    grid_cells_from_parts(
        row.text.as_ref(),
        &row.style_runs,
        max_cols,
        default_fg,
        default_bg,
    )
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn grid_cells_from_parts(
    text: &str,
    style_runs: &[ghostty_vt::StyleRun],
    max_cols: u16,
    default_fg: u32,
    default_bg: u32,
) -> Vec<GridCell> {
    let mut cells: Vec<GridCell> = Vec::new();
    let mut col = 0u16;

    for ch in text.chars() {
        let width = char_cell_width(ch) as u8;
        if width == 0 {
            if let Some(last) = cells.last_mut() {
                last.glyph.push(ch);
            }
            continue;
        }
        if col >= max_cols {
            break;
        }

        let (fg_rgb, bg_rgb, flags) =
            grid_cell_style_at(style_runs, col + 1, default_fg, default_bg);
        cells.push(GridCell {
            col,
            width,
            glyph: ch.to_string(),
            fg_rgb,
            bg_rgb,
            flags,
            kind: if is_geometry_char(ch) {
                GridCellKind::GeometricBlock
            } else {
                GridCellKind::Text
            },
        });
        col = col.saturating_add(width as u16);
    }

    cells
}

pub(super) fn terminal_canvas(
    snapshot: Arc<TerminalSnapshot>,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    config: GridRendererConfig,
    glyph_cache: GlyphCache,
) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _cx: &mut App| {
            let text_system = window.text_system().clone();
            let font_desc = config.cached_font.clone();
            let font_size = px(config.font_size);
            let line_height_px = px(config.cell_height);
            let num_rows = snapshot.rows.len();
            #[cfg(feature = "ghostty_vt")]
            let active_glyph_keys = collect_active_glyph_keys(&snapshot);
            #[cfg(feature = "ghostty_vt")]
            let damaged_glyph_keys = collect_damaged_glyph_keys(&snapshot);
            // Cross-frame glyph cache — lock shared cache from TerminalPane
            // instead of rebuilding HashMap every paint (saves ~50-80% shaping work)
            #[cfg(feature = "ghostty_vt")]
            let mut glyph_layout_cache = {
                // Fast path: read lock for cache hits (vast majority of frames).
                let needs_init = glyph_cache.read().is_empty();
                if needs_init {
                    let mut cache = glyph_cache.write();
                    // Alacritty-inspired: preload ASCII on first paint to eliminate
                    // first-frame shaping latency for common characters.
                    if cache.is_empty() {
                        preload_ascii_glyphs(
                            &text_system,
                            &font_desc,
                            font_size,
                            config.cell_height,
                            &mut cache,
                        );
                        // Prewarm with previously-used glyphs from RAMdisk
                        prewarm_from_ramdisk_keys(
                            &text_system,
                            &font_desc,
                            font_size,
                            config.cell_height,
                            &mut cache,
                        );
                    }
                    prune_glyph_cache(&mut cache, &active_glyph_keys);
                    borrow_active_glyph_plans(&cache, &active_glyph_keys)
                } else {
                    let mut cache = glyph_cache.write();
                    prune_glyph_cache(&mut cache, &active_glyph_keys);
                    borrow_active_glyph_plans(&cache, &active_glyph_keys)
                }
            };
            #[cfg(feature = "ghostty_vt")]
            for key in &damaged_glyph_keys {
                let _ = glyph_layout_for_key(
                    key,
                    &text_system,
                    &font_desc,
                    font_size,
                    config.cell_height,
                    &mut glyph_layout_cache,
                );
            }

            if let Some((sel_start, sel_end)) = selection {
                let max_row = sel_end.row.min(num_rows.saturating_sub(1) as u16);
                for row in sel_start.row..=max_row {
                    let sc = if row == sel_start.row {
                        sel_start.col
                    } else {
                        0
                    };
                    let ec = if row == sel_end.row {
                        sel_end.col
                    } else {
                        config.term_cols
                    };
                    if sc >= ec {
                        continue;
                    }
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                bounds.origin.x
                                    + px(config.horizontal_text_padding
                                        + sc as f32 * config.cell_width),
                                bounds.origin.y + px(row as f32 * config.cell_height),
                            ),
                            size(px((ec - sc) as f32 * config.cell_width), line_height_px),
                        ),
                        rgba(0x2F6FED55),
                    ));
                }
            }

            for (row_idx, row_data) in snapshot.rows.iter().enumerate() {
                let row_y = row_idx as f32 * config.cell_height;
                #[cfg(feature = "ghostty_vt")]
                let glyph_instances = &row_data.glyph_instances;

                #[cfg(feature = "ghostty_vt")]
                for srun in &row_data.style_runs {
                    let bg_val =
                        ((srun.bg.r as u32) << 16) | ((srun.bg.g as u32) << 8) | (srun.bg.b as u32);
                    if bg_val != config.bg_color {
                        let sc = srun.start_col.saturating_sub(1) as f32;
                        let ec = srun.end_col as f32;
                        if ec > sc {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(
                                        bounds.origin.x
                                            + px(config.horizontal_text_padding
                                                + sc * config.cell_width),
                                        bounds.origin.y + px(row_y),
                                    ),
                                    size(px((ec - sc) * config.cell_width), line_height_px),
                                ),
                                Hsla::from(rgb(bg_val)),
                            ));
                        }
                    }
                }

                #[cfg(feature = "ghostty_vt")]
                {
                    for instance in glyph_instances {
                        if matches!(instance.key.render_path, GlyphRenderPath::Geometry) {
                            continue;
                        }
                        let layout = glyph_layout_for_key(
                            &instance.key,
                            &text_system,
                            &font_desc,
                            font_size,
                            config.cell_height,
                            &mut glyph_layout_cache,
                        );
                        paint_glyph_instance(instance, &layout, bounds, &config, window);
                    }
                }

                #[cfg(not(feature = "ghostty_vt"))]
                {
                    let text = &row_data.text;
                    if text.is_empty() {
                        continue;
                    }
                    let shaped_text = sanitize_text_for_shaping(text);
                    let origin = point(
                        bounds.origin.x + px(config.horizontal_text_padding),
                        bounds.origin.y + px(row_y),
                    );
                    let default_fg = Hsla::from(rgb(config.fg_color));
                    let runs = vec![TextRun {
                        len: shaped_text.len(),
                        font: font_desc.clone(),
                        color: default_fg,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }];

                    let has_wide_cells = text.chars().any(|ch| char_cell_width(ch) > 1);
                    let force_width =
                        if !has_wide_cells && text.chars().all(|ch| char_cell_width(ch) <= 1) {
                            Some(px(config.cell_width))
                        } else {
                            None
                        };
                    let shaped = text_system.shape_line(
                        SharedString::from(shaped_text),
                        font_size,
                        &runs,
                        force_width,
                    );
                    let _ = shaped.paint(origin, line_height_px, window, _cx);
                }

                #[cfg(feature = "ghostty_vt")]
                {
                    for cell in &row_data.cells {
                        if cell.kind == GridCellKind::GeometricBlock {
                            paint_geometry_cell(row_idx as u16, cell, bounds, &config, window);
                        }
                    }
                }
            }

            if snapshot.cursor_visible && (snapshot.cursor_y as usize) < num_rows {
                #[cfg(feature = "ghostty_vt")]
                let (cursor_col, cursor_width) = snapshot
                    .rows
                    .get(snapshot.cursor_y as usize)
                    .map(|row| resolve_cursor_cell(snapshot.cursor_x, &row.cells, config.term_cols))
                    .unwrap_or((snapshot.cursor_x.min(config.term_cols.saturating_sub(1)), 1));

                #[cfg(not(feature = "ghostty_vt"))]
                let (cursor_col, cursor_width) =
                    (snapshot.cursor_x.min(config.term_cols.saturating_sub(1)), 1);

                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            bounds.origin.x
                                + px(config.horizontal_text_padding
                                    + cursor_col as f32 * config.cell_width),
                            bounds.origin.y + px(snapshot.cursor_y as f32 * config.cell_height),
                        ),
                        size(px(config.cell_width * cursor_width as f32), line_height_px),
                    ),
                    rgba(0xF5F5F780),
                ));
            }

            #[cfg(feature = "ghostty_vt")]
            {
                let mut cache = glyph_cache.write();
                prune_glyph_cache(&mut cache, &active_glyph_keys);
                persist_active_glyph_plans(&mut cache, glyph_layout_cache, &active_glyph_keys);
            }
        },
    )
    .size_full()
}

#[cfg(all(test, feature = "ghostty_vt"))]
fn build_canvas_text_runs(
    text: &str,
    style_runs: &[ghostty_vt::StyleRun],
    font_desc: &Font,
    default_fg: Hsla,
) -> Vec<TextRun> {
    if style_runs.is_empty() || text.is_empty() {
        return vec![TextRun {
            len: text.len(),
            font: font_desc.clone(),
            color: default_fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
    }

    let char_byte_offsets: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();

    let mut runs: Vec<TextRun> = Vec::new();
    let mut covered_to_byte: usize = 0;

    for run in style_runs {
        let start_char = col_to_char_index(text, run.start_col.saturating_sub(1) as usize);
        let end_char = col_to_char_index(text, run.end_col as usize);
        if start_char >= char_byte_offsets.len().saturating_sub(1) || start_char >= end_char {
            continue;
        }

        let byte_start = char_byte_offsets[start_char];
        let byte_end = char_byte_offsets[end_char.min(char_byte_offsets.len() - 1)];

        if byte_start < covered_to_byte {
            continue;
        }

        if covered_to_byte < byte_start {
            runs.push(TextRun {
                len: byte_start - covered_to_byte,
                font: font_desc.clone(),
                color: default_fg,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }

        let fg_val = ((run.fg.r as u32) << 16) | ((run.fg.g as u32) << 8) | (run.fg.b as u32);
        let fg_color = Hsla::from(rgb(fg_val));

        let mut run_font = font_desc.clone();
        if run.flags & GHOSTTY_FLAG_BOLD != 0 {
            run_font.weight = FontWeight::BOLD;
        }
        if run.flags & GHOSTTY_FLAG_ITALIC != 0 {
            run_font.style = FontStyle::Italic;
        }

        runs.push(TextRun {
            len: byte_end - byte_start,
            font: run_font,
            color: fg_color,
            background_color: None,
            underline: if run.flags & GHOSTTY_FLAG_UNDERLINE != 0 {
                Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(fg_color),
                    wavy: false,
                })
            } else {
                None
            },
            strikethrough: if run.flags & GHOSTTY_FLAG_STRIKETHROUGH != 0 {
                Some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(fg_color),
                })
            } else {
                None
            },
        });
        covered_to_byte = byte_end;
    }

    if covered_to_byte < text.len() {
        runs.push(TextRun {
            len: text.len() - covered_to_byte,
            font: font_desc.clone(),
            color: default_fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }

    runs
}

#[cfg(all(test, feature = "ghostty_vt"))]
mod tests {
    use super::*;

    fn sample_style_run(flags: u8) -> ghostty_vt::StyleRun {
        ghostty_vt::StyleRun {
            start_col: 1,
            end_col: 3,
            fg: ghostty_vt::Rgb {
                r: 0xFF,
                g: 0x88,
                b: 0x33,
            },
            bg: ghostty_vt::Rgb {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
            flags,
        }
    }

    #[::core::prelude::v1::test]
    fn build_canvas_text_runs_uses_ghostty_flag_layout() {
        let runs = build_canvas_text_runs(
            "abc",
            &[sample_style_run(
                GHOSTTY_FLAG_BOLD
                    | GHOSTTY_FLAG_ITALIC
                    | GHOSTTY_FLAG_UNDERLINE
                    | GHOSTTY_FLAG_STRIKETHROUGH,
            )],
            &font("Consolas"),
            Hsla::from(rgb(DEFAULT_FG)),
        );

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].font.weight, FontWeight::BOLD);
        assert_eq!(runs[0].font.style, FontStyle::Italic);
        assert!(runs[0].underline.is_some());
        assert!(runs[0].strikethrough.is_some());
    }

    #[::core::prelude::v1::test]
    fn build_canvas_text_runs_does_not_confuse_italic_with_underline() {
        let runs = build_canvas_text_runs(
            "abc",
            &[sample_style_run(GHOSTTY_FLAG_ITALIC)],
            &font("Consolas"),
            Hsla::from(rgb(DEFAULT_FG)),
        );

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].font.style, FontStyle::Italic);
        assert!(runs[0].underline.is_none());
        assert!(runs[0].strikethrough.is_none());
    }

    #[::core::prelude::v1::test]
    fn char_cell_width_treats_wide_glyphs_as_two_cells() {
        assert_eq!(char_cell_width('a'), 1);
        assert_eq!(char_cell_width('あ'), 2);
        assert_eq!(char_cell_width('🔥'), 2);
        assert_eq!(char_cell_width('\u{FE0F}'), 0);
    }

    #[::core::prelude::v1::test]
    fn sanitize_text_for_shaping_replaces_block_glyphs_only() {
        let shaped = sanitize_text_for_shaping("A█▀B");
        assert_eq!(shaped, format!("A{BRAILLE_BLANK}{BRAILLE_BLANK}B"));
    }

    #[::core::prelude::v1::test]
    fn build_canvas_text_runs_maps_style_runs_by_terminal_columns() {
        let runs = build_canvas_text_runs(
            "🔥a",
            &[ghostty_vt::StyleRun {
                start_col: 1,
                end_col: 2,
                fg: ghostty_vt::Rgb {
                    r: 0xFF,
                    g: 0x88,
                    b: 0x33,
                },
                bg: ghostty_vt::Rgb {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                },
                flags: GHOSTTY_FLAG_UNDERLINE,
            }],
            &font("Consolas"),
            Hsla::from(rgb(DEFAULT_FG)),
        );

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, "🔥".len());
        assert!(runs[0].underline.is_some());
        assert!(runs[1].underline.is_none());
    }

    #[::core::prelude::v1::test]
    fn grid_cells_from_row_merges_zero_width_with_previous_cell() {
        let row = CachedTerminalRow {
            text: SharedString::from("🔥\u{FE0F}a"),
            style_runs: vec![
                ghostty_vt::StyleRun {
                    start_col: 1,
                    end_col: 2,
                    fg: ghostty_vt::Rgb {
                        r: 0xAA,
                        g: 0x00,
                        b: 0x00,
                    },
                    bg: ghostty_vt::Rgb {
                        r: 0x00,
                        g: 0x00,
                        b: 0x00,
                    },
                    flags: GHOSTTY_FLAG_BOLD,
                },
                ghostty_vt::StyleRun {
                    start_col: 3,
                    end_col: 3,
                    fg: ghostty_vt::Rgb {
                        r: 0x00,
                        g: 0xAA,
                        b: 0x00,
                    },
                    bg: ghostty_vt::Rgb {
                        r: 0x00,
                        g: 0x00,
                        b: 0x00,
                    },
                    flags: 0,
                },
            ],
            cells: Vec::new(),
            glyph_instances: Vec::new(),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let cells = grid_cells_from_row(&row, 10, DEFAULT_FG, DEFAULT_BG);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].glyph, "🔥\u{FE0F}");
        assert_eq!(cells[0].width, 2);
        assert_eq!(cells[0].col, 0);
        assert_eq!(cells[0].flags, GHOSTTY_FLAG_BOLD);
        assert_eq!(cells[1].glyph, "a");
        assert_eq!(cells[1].col, 2);
    }

    #[::core::prelude::v1::test]
    fn grid_cells_from_row_marks_geometric_blocks() {
        let row = CachedTerminalRow {
            text: SharedString::from("█a"),
            style_runs: vec![ghostty_vt::StyleRun {
                start_col: 1,
                end_col: 2,
                fg: ghostty_vt::Rgb {
                    r: 0xFF,
                    g: 0x88,
                    b: 0x33,
                },
                bg: ghostty_vt::Rgb {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                },
                flags: 0,
            }],
            cells: Vec::new(),
            glyph_instances: Vec::new(),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let cells = grid_cells_from_row(&row, 10, DEFAULT_FG, DEFAULT_BG);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].kind, GridCellKind::GeometricBlock);
        assert_eq!(cells[1].kind, GridCellKind::Text);
    }

    #[::core::prelude::v1::test]
    fn grid_cells_from_row_marks_box_drawing_as_geometry() {
        let row = CachedTerminalRow {
            text: SharedString::from("─═│┌a"),
            style_runs: vec![ghostty_vt::StyleRun {
                start_col: 1,
                end_col: 5,
                fg: ghostty_vt::Rgb {
                    r: 0xFF,
                    g: 0x88,
                    b: 0x33,
                },
                bg: ghostty_vt::Rgb {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                },
                flags: 0,
            }],
            cells: Vec::new(),
            glyph_instances: Vec::new(),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let cells = grid_cells_from_row(&row, 10, DEFAULT_FG, DEFAULT_BG);
        assert_eq!(cells.len(), 5);
        assert_eq!(cells[0].kind, GridCellKind::GeometricBlock);
        assert_eq!(cells[1].kind, GridCellKind::GeometricBlock);
        assert_eq!(cells[2].kind, GridCellKind::GeometricBlock);
        assert_eq!(cells[3].kind, GridCellKind::GeometricBlock);
        assert_eq!(cells[4].kind, GridCellKind::Text);
    }

    #[::core::prelude::v1::test]
    fn grid_cells_from_parts_matches_row_wrapper() {
        let row = CachedTerminalRow {
            text: SharedString::from("A🔥"),
            style_runs: vec![ghostty_vt::StyleRun {
                start_col: 1,
                end_col: 3,
                fg: ghostty_vt::Rgb {
                    r: 0xAA,
                    g: 0xBB,
                    b: 0xCC,
                },
                bg: ghostty_vt::Rgb {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                },
                flags: GHOSTTY_FLAG_UNDERLINE,
            }],
            cells: Vec::new(),
            glyph_instances: Vec::new(),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        assert_eq!(
            grid_cells_from_parts(
                row.text.as_ref(),
                &row.style_runs,
                10,
                DEFAULT_FG,
                DEFAULT_BG
            ),
            grid_cells_from_row(&row, 10, DEFAULT_FG, DEFAULT_BG)
        );
    }

    #[::core::prelude::v1::test]
    fn glyph_key_from_cell_classifies_render_paths() {
        let mono = GridCell {
            col: 0,
            width: 1,
            glyph: "A".into(),
            fg_rgb: 0,
            bg_rgb: 0,
            flags: 0,
            kind: GridCellKind::Text,
        };
        let emoji = GridCell {
            col: 1,
            width: 2,
            glyph: "🔥\u{FE0F}".into(),
            fg_rgb: 0,
            bg_rgb: 0,
            flags: 0,
            kind: GridCellKind::Text,
        };
        let block = GridCell {
            col: 2,
            width: 1,
            glyph: "█".into(),
            fg_rgb: 0,
            bg_rgb: 0,
            flags: 0,
            kind: GridCellKind::GeometricBlock,
        };

        assert_eq!(
            glyph_key_from_cell(&mono).render_path,
            GlyphRenderPath::AtlasMonochrome
        );
        assert_eq!(
            glyph_key_from_cell(&emoji).render_path,
            GlyphRenderPath::AtlasPolychrome
        );
        assert_eq!(
            glyph_key_from_cell(&block).render_path,
            GlyphRenderPath::Geometry
        );
    }

    #[::core::prelude::v1::test]
    fn glyph_instances_from_cells_preserves_grid_positions() {
        let cells = vec![
            GridCell {
                col: 0,
                width: 2,
                glyph: "🔥\u{FE0F}".into(),
                fg_rgb: 0x112233,
                bg_rgb: 0x000000,
                flags: GHOSTTY_FLAG_BOLD,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 2,
                width: 1,
                glyph: "█".into(),
                fg_rgb: 0x445566,
                bg_rgb: 0x000000,
                flags: 0,
                kind: GridCellKind::GeometricBlock,
            },
        ];

        let instances = glyph_instances_from_cells(&cells, 7);
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].row, 7);
        assert_eq!(instances[0].col, 0);
        assert_eq!(
            instances[0].key.render_path,
            GlyphRenderPath::AtlasPolychrome
        );
        assert_eq!(instances[1].col, 2);
        assert_eq!(instances[1].key.render_path, GlyphRenderPath::Geometry);
    }

    #[::core::prelude::v1::test]
    fn glyph_instances_from_row_uses_cached_cells() {
        let row = CachedTerminalRow {
            text: SharedString::from("ignored"),
            style_runs: Vec::new(),
            cells: vec![
                GridCell {
                    col: 0,
                    width: 2,
                    glyph: "🔥\u{FE0F}".into(),
                    fg_rgb: 0x112233,
                    bg_rgb: 0,
                    flags: 0,
                    kind: GridCellKind::Text,
                },
                GridCell {
                    col: 2,
                    width: 1,
                    glyph: "█".into(),
                    fg_rgb: 0x445566,
                    bg_rgb: 0,
                    flags: 0,
                    kind: GridCellKind::GeometricBlock,
                },
            ],
            glyph_instances: Vec::new(),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let instances = glyph_instances_from_row(&row, 5);
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].row, 5);
        assert_eq!(instances[0].col, 0);
        assert_eq!(instances[1].col, 2);
        assert_eq!(instances[1].key.render_path, GlyphRenderPath::Geometry);
    }

    #[::core::prelude::v1::test]
    fn collect_active_glyph_keys_deduplicates_and_skips_geometry() {
        let mono_key = GlyphKey {
            glyph: "A".into(),
            width: 1,
            flags: 0,
            render_path: GlyphRenderPath::AtlasMonochrome,
        };
        let geometry_key = GlyphKey {
            glyph: "█".into(),
            width: 1,
            flags: 0,
            render_path: GlyphRenderPath::Geometry,
        };
        let snapshot = TerminalSnapshot {
            rows: vec![CachedTerminalRow {
                text: SharedString::new(""),
                style_runs: Vec::new(),
                cells: Vec::new(),
                glyph_instances: vec![
                    GlyphInstance {
                        row: 0,
                        col: 0,
                        fg_rgb: 0,
                        bg_rgb: 0,
                        key: mono_key.clone(),
                    },
                    GlyphInstance {
                        row: 0,
                        col: 1,
                        fg_rgb: 0,
                        bg_rgb: 0,
                        key: mono_key.clone(),
                    },
                    GlyphInstance {
                        row: 0,
                        col: 2,
                        fg_rgb: 0,
                        bg_rgb: 0,
                        key: geometry_key,
                    },
                ],
                damage_spans: Vec::new(),
                damaged_glyph_instances: Vec::new(),
            }],
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: true,
            content_revision: 0,
            damaged_rows: Vec::new(),
            prev_damaged_rows: Vec::new(),
        };

        let keys = collect_active_glyph_keys(&snapshot);
        assert_eq!(keys.len(), 1);
        assert!(keys.contains(&mono_key));
    }

    #[::core::prelude::v1::test]
    fn collect_damaged_glyph_keys_uses_only_damage_payload() {
        let active_key = GlyphKey {
            glyph: "A".into(),
            width: 1,
            flags: 0,
            render_path: GlyphRenderPath::AtlasMonochrome,
        };
        let damaged_key = GlyphKey {
            glyph: "X".into(),
            width: 1,
            flags: GHOSTTY_FLAG_BOLD,
            render_path: GlyphRenderPath::AtlasMonochrome,
        };
        let snapshot = TerminalSnapshot {
            rows: vec![CachedTerminalRow {
                text: SharedString::new(""),
                style_runs: Vec::new(),
                cells: Vec::new(),
                glyph_instances: vec![GlyphInstance {
                    row: 0,
                    col: 0,
                    fg_rgb: 0,
                    bg_rgb: 0,
                    key: active_key,
                }],
                damage_spans: vec![DamageSpan {
                    start_col: 1,
                    end_col: 2,
                }],
                damaged_glyph_instances: vec![GlyphInstance {
                    row: 0,
                    col: 1,
                    fg_rgb: 0,
                    bg_rgb: 0,
                    key: damaged_key.clone(),
                }],
            }],
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: true,
            content_revision: 0,
            damaged_rows: Vec::new(),
            prev_damaged_rows: Vec::new(),
        };

        let keys = collect_damaged_glyph_keys(&snapshot);
        assert_eq!(keys, HashSet::from([damaged_key]));
    }

    #[::core::prelude::v1::test]
    fn borrow_and_persist_active_glyph_plans_touch_only_active_keys() {
        let active_key = GlyphKey {
            glyph: "A".into(),
            width: 1,
            flags: 0,
            render_path: GlyphRenderPath::AtlasMonochrome,
        };
        let stale_key = GlyphKey {
            glyph: "B".into(),
            width: 1,
            flags: 0,
            render_path: GlyphRenderPath::AtlasMonochrome,
        };
        let mut shared = HashMap::from([
            (active_key.clone(), PreparedGlyphPlan::default()),
            (stale_key.clone(), PreparedGlyphPlan::default()),
        ]);
        let active_keys = HashSet::from([active_key.clone()]);

        let mut local = borrow_active_glyph_plans(&shared, &active_keys);
        local.insert(
            GlyphKey {
                glyph: "X".into(),
                width: 1,
                flags: GHOSTTY_FLAG_BOLD,
                render_path: GlyphRenderPath::AtlasMonochrome,
            },
            PreparedGlyphPlan::default(),
        );
        prune_glyph_cache(&mut shared, &active_keys);
        persist_active_glyph_plans(&mut shared, local, &active_keys);

        assert_eq!(shared.len(), 1);
        assert!(shared.contains_key(&active_key));
        assert!(!shared.contains_key(&stale_key));
    }

    #[::core::prelude::v1::test]
    fn prune_glyph_cache_keeps_only_active_keys() {
        let active_key = GlyphKey {
            glyph: "A".into(),
            width: 1,
            flags: 0,
            render_path: GlyphRenderPath::AtlasMonochrome,
        };
        let stale_key = GlyphKey {
            glyph: "B".into(),
            width: 1,
            flags: 0,
            render_path: GlyphRenderPath::AtlasMonochrome,
        };

        let mut cache = HashMap::from([
            (active_key.clone(), PreparedGlyphPlan::default()),
            (stale_key.clone(), PreparedGlyphPlan::default()),
        ]);
        let active = HashSet::from([active_key.clone()]);

        prune_glyph_cache(&mut cache, &active);

        assert_eq!(cache.len(), 1);
        assert!(cache.contains_key(&active_key));
        assert!(!cache.contains_key(&stale_key));
    }

    #[::core::prelude::v1::test]
    fn damage_spans_from_cells_tracks_only_changed_columns() {
        let previous = vec![
            GridCell {
                col: 0,
                width: 1,
                glyph: "A".into(),
                fg_rgb: 0x111111,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 1,
                width: 1,
                glyph: "B".into(),
                fg_rgb: 0x222222,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 2,
                width: 1,
                glyph: "C".into(),
                fg_rgb: 0x333333,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
        ];
        let next = vec![
            GridCell {
                col: 0,
                width: 1,
                glyph: "A".into(),
                fg_rgb: 0x111111,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 1,
                width: 1,
                glyph: "X".into(),
                fg_rgb: 0x777777,
                bg_rgb: 0,
                flags: GHOSTTY_FLAG_BOLD,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 2,
                width: 1,
                glyph: "C".into(),
                fg_rgb: 0x333333,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
        ];

        assert_eq!(
            damage_spans_from_cells(&previous, &next, 4),
            vec![DamageSpan {
                start_col: 1,
                end_col: 2,
            }]
        );
    }

    #[::core::prelude::v1::test]
    fn damage_spans_from_cells_expands_across_wide_glyph_columns() {
        let previous = vec![GridCell {
            col: 0,
            width: 2,
            glyph: "🔥\u{FE0F}".into(),
            fg_rgb: 0,
            bg_rgb: 0,
            flags: 0,
            kind: GridCellKind::Text,
        }];
        let next = vec![
            GridCell {
                col: 0,
                width: 1,
                glyph: "A".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 1,
                width: 1,
                glyph: "B".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
        ];

        assert_eq!(
            damage_spans_from_cells(&previous, &next, 4),
            vec![DamageSpan {
                start_col: 0,
                end_col: 2,
            }]
        );
    }

    #[::core::prelude::v1::test]
    fn damage_spans_from_style_runs_detects_blank_background_changes() {
        let previous = Vec::new();
        let next = vec![ghostty_vt::StyleRun {
            start_col: 3,
            end_col: 4,
            fg: ghostty_vt::Rgb {
                r: 0xAA,
                g: 0xAA,
                b: 0xAA,
            },
            bg: ghostty_vt::Rgb {
                r: 0x22,
                g: 0x44,
                b: 0x66,
            },
            flags: 0,
        }];

        assert_eq!(
            damage_spans_from_style_runs(&previous, &next, 6, DEFAULT_FG, DEFAULT_BG),
            vec![DamageSpan {
                start_col: 2,
                end_col: 4,
            }]
        );
    }

    #[::core::prelude::v1::test]
    fn damage_spans_from_terminal_row_merges_cell_and_style_damage() {
        let previous_cells = vec![GridCell {
            col: 0,
            width: 1,
            glyph: "A".into(),
            fg_rgb: 0x111111,
            bg_rgb: 0,
            flags: 0,
            kind: GridCellKind::Text,
        }];
        let next_cells = vec![GridCell {
            col: 0,
            width: 1,
            glyph: "X".into(),
            fg_rgb: 0x111111,
            bg_rgb: 0,
            flags: 0,
            kind: GridCellKind::Text,
        }];
        let previous_styles = Vec::new();
        let next_styles = vec![ghostty_vt::StyleRun {
            start_col: 4,
            end_col: 4,
            fg: ghostty_vt::Rgb {
                r: 0xAA,
                g: 0xAA,
                b: 0xAA,
            },
            bg: ghostty_vt::Rgb {
                r: 0x22,
                g: 0x44,
                b: 0x66,
            },
            flags: 0,
        }];

        assert_eq!(
            damage_spans_from_terminal_row(
                &previous_cells,
                &previous_styles,
                &next_cells,
                &next_styles,
                6,
                DEFAULT_FG,
                DEFAULT_BG,
            ),
            vec![
                DamageSpan {
                    start_col: 0,
                    end_col: 1,
                },
                DamageSpan {
                    start_col: 3,
                    end_col: 4,
                },
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn patch_cells_in_damage_reuses_unchanged_neighbors() {
        let previous = vec![
            GridCell {
                col: 0,
                width: 1,
                glyph: "A".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 1,
                width: 1,
                glyph: "B".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 2,
                width: 1,
                glyph: "C".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
        ];
        let next = vec![
            GridCell {
                col: 0,
                width: 1,
                glyph: "A".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 1,
                width: 1,
                glyph: "X".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: GHOSTTY_FLAG_UNDERLINE,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 2,
                width: 1,
                glyph: "C".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
        ];
        let damage = vec![DamageSpan {
            start_col: 1,
            end_col: 2,
        }];

        let patched = patch_cells_in_damage(&previous, &next, &damage);

        assert_eq!(patched.len(), 3);
        assert_eq!(patched[0].glyph, "A");
        assert_eq!(patched[1].glyph, "X");
        assert_eq!(patched[2].glyph, "C");
    }

    #[::core::prelude::v1::test]
    fn patch_glyph_instances_in_damage_replaces_only_changed_segment() {
        let previous_cells = vec![
            GridCell {
                col: 0,
                width: 1,
                glyph: "A".into(),
                fg_rgb: 0x111111,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 1,
                width: 1,
                glyph: "B".into(),
                fg_rgb: 0x222222,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 2,
                width: 1,
                glyph: "C".into(),
                fg_rgb: 0x333333,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
        ];
        let next_cells = vec![
            GridCell {
                col: 0,
                width: 1,
                glyph: "A".into(),
                fg_rgb: 0x111111,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 1,
                width: 1,
                glyph: "X".into(),
                fg_rgb: 0x999999,
                bg_rgb: 0,
                flags: GHOSTTY_FLAG_BOLD,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 2,
                width: 1,
                glyph: "C".into(),
                fg_rgb: 0x333333,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
        ];
        let previous_instances = glyph_instances_from_cells(&previous_cells, 4);
        let damage = vec![DamageSpan {
            start_col: 1,
            end_col: 2,
        }];

        let patched = patch_glyph_instances_in_damage(&previous_instances, &next_cells, 4, &damage);

        assert_eq!(patched.len(), 3);
        assert_eq!(patched[0].col, 0);
        assert_eq!(patched[1].col, 1);
        assert_eq!(patched[1].fg_rgb, 0x999999);
        assert_eq!(patched[1].key.glyph, "X");
        assert_eq!(patched[2].col, 2);
    }

    #[::core::prelude::v1::test]
    fn resolve_cursor_cell_snaps_wide_tail_to_head() {
        let cells = vec![
            GridCell {
                col: 0,
                width: 2,
                glyph: "🔥\u{FE0F}".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
            GridCell {
                col: 2,
                width: 1,
                glyph: "a".into(),
                fg_rgb: 0,
                bg_rgb: 0,
                flags: 0,
                kind: GridCellKind::Text,
            },
        ];

        assert_eq!(resolve_cursor_cell(0, &cells, 10), (0, 2));
        assert_eq!(resolve_cursor_cell(1, &cells, 10), (0, 2));
        assert_eq!(resolve_cursor_cell(2, &cells, 10), (2, 1));
    }

    #[::core::prelude::v1::test]
    fn resolve_cursor_cell_falls_back_to_single_blank_cell() {
        let cells = vec![GridCell {
            col: 0,
            width: 1,
            glyph: "a".into(),
            fg_rgb: 0,
            bg_rgb: 0,
            flags: 0,
            kind: GridCellKind::Text,
        }];

        assert_eq!(resolve_cursor_cell(4, &cells, 10), (4, 1));
        assert_eq!(resolve_cursor_cell(99, &cells, 10), (9, 1));
    }

    #[::core::prelude::v1::test]
    fn text_run_for_glyph_key_applies_font_style_flags() {
        let key = GlyphKey {
            glyph: "A".into(),
            width: 1,
            flags: GHOSTTY_FLAG_BOLD | GHOSTTY_FLAG_ITALIC,
            render_path: GlyphRenderPath::AtlasMonochrome,
        };

        let run = text_run_for_glyph_key(&key, &font("Consolas"), Hsla::from(rgb(DEFAULT_FG)));
        assert_eq!(run.font.weight, FontWeight::BOLD);
        assert_eq!(run.font.style, FontStyle::Italic);
        assert_eq!(run.len, 1);
    }

    #[::core::prelude::v1::test]
    fn glyph_baseline_y_centers_layout_within_cell_height() {
        let layout = LineLayout {
            font_size: px(14.0),
            width: px(8.0),
            ascent: px(9.0),
            descent: px(3.0),
            runs: Vec::new(),
            len: 1,
        };

        assert_eq!(glyph_baseline_y(&layout, 16.0), px(11.0));
    }
}
