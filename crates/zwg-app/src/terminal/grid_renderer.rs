use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::*;
use parking_lot::Mutex;
use unicode_width::UnicodeWidthChar;

use super::{DEFAULT_BG, DEFAULT_FG};

/// Cross-frame glyph layout cache type.
/// Persists shaped glyphs across render frames to avoid redundant text shaping.
#[cfg(feature = "ghostty_vt")]
pub(super) type GlyphCache = Arc<Mutex<HashMap<GlyphKey, PreparedGlyphPlan>>>;

#[cfg(not(feature = "ghostty_vt"))]
pub(super) type GlyphCache = ();

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
}

impl TerminalSnapshot {
    pub fn new(rows: u16) -> Self {
        Self {
            rows: vec![CachedTerminalRow::default(); rows as usize],
            cursor_x: 0,
            cursor_y: 0,
        }
    }

    pub fn resize(&mut self, rows: u16) {
        self.rows
            .resize(rows as usize, CachedTerminalRow::default());
    }
}

pub(super) struct GridRendererConfig {
    pub cell_width: f32,
    pub cell_height: f32,
    pub font_family: &'static str,
    pub font_size: f32,
    pub horizontal_text_padding: f32,
    pub term_cols: u16,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GlyphRenderPath {
    AtlasMonochrome,
    AtlasPolychrome,
    Geometry,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

pub(crate) fn sanitize_text_for_shaping(text: &str) -> String {
    text.chars()
        .map(|ch| if is_geometric_block_char(ch) { BRAILLE_BLANK } else { ch })
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
    cells.iter()
        .map(|cell| GlyphInstance {
            row,
            col: cell.col,
            fg_rgb: cell.fg_rgb,
            bg_rgb: cell.bg_rgb,
            key: glyph_key_from_cell(cell),
        })
        .collect()
}

#[cfg(feature = "ghostty_vt")]
fn spans_overlap(start: u16, end: u16, span: DamageSpan) -> bool {
    start < span.end_col && span.start_col < end
}

#[cfg(feature = "ghostty_vt")]
fn cell_overlaps_damage(cell: &GridCell, spans: &[DamageSpan]) -> bool {
    let start = cell.col;
    let end = cell.col.saturating_add(cell.width as u16);
    spans.iter().copied().any(|span| spans_overlap(start, end, span))
}

#[cfg(feature = "ghostty_vt")]
fn instance_overlaps_damage(instance: &GlyphInstance, spans: &[DamageSpan]) -> bool {
    let start = instance.col;
    let end = instance.col.saturating_add(instance.key.width as u16);
    spans.iter().copied().any(|span| spans_overlap(start, end, span))
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
fn column_damage_signatures(cells: &[GridCell], max_cols: u16) -> Vec<Option<ColumnDamageSignature>> {
    let mut columns = vec![None; max_cols as usize];
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
) -> Vec<StyleDamageSignature> {
    (0..term_cols)
        .map(|col| {
            let (fg_rgb, bg_rgb, flags) = grid_cell_style_at(style_runs, col + 1);
            StyleDamageSignature {
                fg_rgb,
                bg_rgb,
                flags,
            }
        })
        .collect()
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
    let mut spans = Vec::new();
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
) -> Vec<DamageSpan> {
    let previous_columns = style_damage_signatures(previous, term_cols);
    let next_columns = style_damage_signatures(next, term_cols);
    let mut spans = Vec::new();
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
) -> Vec<DamageSpan> {
    merge_damage_spans(
        damage_spans_from_cells(previous_cells, next_cells, term_cols)
            .into_iter()
            .chain(damage_spans_from_style_runs(previous_styles, next_styles, term_cols))
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

    let mut cells: Vec<GridCell> = previous
        .iter()
        .filter(|cell| !cell_overlaps_damage(cell, damage_spans))
        .cloned()
        .collect();
    cells.extend(
        next.iter()
            .filter(|cell| cell_overlaps_damage(cell, damage_spans))
            .cloned(),
    );
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

    let mut instances: Vec<GlyphInstance> = previous
        .iter()
        .filter(|instance| !instance_overlaps_damage(instance, damage_spans))
        .cloned()
        .collect();
    instances.extend(glyph_instances_in_damage(next_cells, row, damage_spans));
    instances.sort_unstable_by_key(|instance| instance.col);
    instances
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn glyph_instances_from_row(row: &CachedTerminalRow, row_idx: u16) -> Vec<GlyphInstance> {
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
    active_keys
        .iter()
        .filter_map(|key| cache.get(key).cloned().map(|plan| (key.clone(), plan)))
        .collect()
}

#[cfg(feature = "ghostty_vt")]
fn persist_active_glyph_plans(
    cache: &mut HashMap<GlyphKey, PreparedGlyphPlan>,
    local_plans: &HashMap<GlyphKey, PreparedGlyphPlan>,
    active_keys: &HashSet<GlyphKey>,
) {
    for key in active_keys {
        if let Some(plan) = local_plans.get(key) {
            cache.insert(key.clone(), plan.clone());
        }
    }
}

#[cfg(feature = "ghostty_vt")]
fn prune_glyph_cache(cache: &mut HashMap<GlyphKey, PreparedGlyphPlan>, active_keys: &HashSet<GlyphKey>) {
    cache.retain(|key, _| active_keys.contains(key));
}

#[cfg(feature = "ghostty_vt")]
fn resolve_cursor_cell(cursor_col: u16, row_cells: &[GridCell], term_cols: u16) -> (u16, u8) {
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
            let shaped = text_system.shape_line(SharedString::from(key.glyph.clone()), font_size, &[run], None);
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
        bounds.origin.x + px(config.horizontal_text_padding + instance.col as f32 * config.cell_width),
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

#[cfg(feature = "ghostty_vt")]
fn default_grid_cell_style() -> (u32, u32, u8) {
    (DEFAULT_FG, DEFAULT_BG, 0)
}

#[cfg(feature = "ghostty_vt")]
fn grid_cell_style_at(style_runs: &[ghostty_vt::StyleRun], col: u16) -> (u32, u32, u8) {
    style_runs
        .iter()
        .find(|run| run.start_col <= col && col <= run.end_col)
        .map(|run| {
            let fg = ((run.fg.r as u32) << 16) | ((run.fg.g as u32) << 8) | (run.fg.b as u32);
            let bg = ((run.bg.r as u32) << 16) | ((run.bg.g as u32) << 8) | (run.bg.b as u32);
            (fg, bg, run.flags)
        })
        .unwrap_or_else(default_grid_cell_style)
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn grid_cells_from_row(row: &CachedTerminalRow, max_cols: u16) -> Vec<GridCell> {
    grid_cells_from_parts(row.text.as_ref(), &row.style_runs, max_cols)
}

#[cfg(feature = "ghostty_vt")]
pub(crate) fn grid_cells_from_parts(
    text: &str,
    style_runs: &[ghostty_vt::StyleRun],
    max_cols: u16,
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

        let (fg_rgb, bg_rgb, flags) = grid_cell_style_at(style_runs, col + 1);
        cells.push(GridCell {
            col,
            width,
            glyph: ch.to_string(),
            fg_rgb,
            bg_rgb,
            flags,
            kind: if is_geometric_block_char(ch) {
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
    snapshot: TerminalSnapshot,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    config: GridRendererConfig,
    glyph_cache: GlyphCache,
) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _cx: &mut App| {
            let text_system = window.text_system().clone();
            let font_desc = font(config.font_family);
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
                let mut cache = glyph_cache.lock();
                prune_glyph_cache(&mut cache, &active_glyph_keys);
                borrow_active_glyph_plans(&cache, &active_glyph_keys)
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

            window.paint_quad(fill(bounds, Hsla::from(rgb(DEFAULT_BG))));

            if let Some((sel_start, sel_end)) = selection {
                let max_row = sel_end.row.min(num_rows.saturating_sub(1) as u16);
                for row in sel_start.row..=max_row {
                    let sc = if row == sel_start.row { sel_start.col } else { 0 };
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
                                    + px(
                                        config.horizontal_text_padding
                                            + sc as f32 * config.cell_width,
                                    ),
                                bounds.origin.y + px(row as f32 * config.cell_height),
                            ),
                            size(
                                px((ec - sc) as f32 * config.cell_width),
                                line_height_px,
                            ),
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
                    let bg_val = ((srun.bg.r as u32) << 16)
                        | ((srun.bg.g as u32) << 8)
                        | (srun.bg.b as u32);
                    if bg_val != DEFAULT_BG {
                        let sc = srun.start_col.saturating_sub(1) as f32;
                        let ec = srun.end_col as f32;
                        if ec > sc {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(
                                        bounds.origin.x
                                            + px(
                                                config.horizontal_text_padding
                                                    + sc * config.cell_width,
                                            ),
                                        bounds.origin.y + px(row_y),
                                    ),
                                    size(
                                        px((ec - sc) * config.cell_width),
                                        line_height_px,
                                    ),
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
                    let default_fg = Hsla::from(rgb(DEFAULT_FG));
                    let runs = vec![TextRun {
                        len: shaped_text.len(),
                        font: font_desc.clone(),
                        color: default_fg,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }];

                    let has_wide_cells = text.chars().any(|ch| char_cell_width(ch) > 1);
                    let force_width = if !has_wide_cells && text.chars().all(|ch| char_cell_width(ch) <= 1)
                    {
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
                    for instance in glyph_instances {
                        if instance.key.render_path == GlyphRenderPath::Geometry {
                            let fg = Hsla::from(rgb(instance.fg_rgb));
                            let x = bounds.origin.x
                                + px(
                                    config.horizontal_text_padding
                                        + instance.col as f32 * config.cell_width,
                                );
                            let y = bounds.origin.y
                                + px(instance.row as f32 * config.cell_height);
                            let cw = px(config.cell_width * instance.key.width as f32);
                            let ch = instance.key.glyph.chars().next().unwrap_or(' ');
                            match ch {
                                '\u{2588}' => {
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, line_height_px)),
                                        fg,
                                    ));
                                }
                                '\u{2580}' => {
                                    let h = px(config.cell_height * 0.5);
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, h)),
                                        fg,
                                    ));
                                }
                                '\u{2584}' => {
                                    let h = px(config.cell_height * 0.5);
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y + h), size(cw, h)),
                                        fg,
                                    ));
                                }
                                '\u{258C}' => {
                                    let hw = px(config.cell_width * 0.5);
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(hw, line_height_px)),
                                        fg,
                                    ));
                                }
                                '\u{2590}' => {
                                    let hw = px(config.cell_width * 0.5);
                                    window.paint_quad(fill(
                                        Bounds::new(point(x + hw, y), size(hw, line_height_px)),
                                        fg,
                                    ));
                                }
                                '\u{2591}' => {
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, line_height_px)),
                                        fg.opacity(0.25),
                                    ));
                                }
                                '\u{2592}' => {
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, line_height_px)),
                                        fg.opacity(0.5),
                                    ));
                                }
                                '\u{2593}' => {
                                    window.paint_quad(fill(
                                        Bounds::new(point(x, y), size(cw, line_height_px)),
                                        fg.opacity(0.75),
                                    ));
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            if (snapshot.cursor_y as usize) < num_rows {
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
                                + px(
                                    config.horizontal_text_padding
                                        + cursor_col as f32 * config.cell_width,
                                ),
                            bounds.origin.y + px(snapshot.cursor_y as f32 * config.cell_height),
                        ),
                        size(px(config.cell_width * cursor_width as f32), line_height_px),
                    ),
                    rgba(0xF5F5F780),
                ));
            }

            #[cfg(feature = "ghostty_vt")]
            {
                let mut cache = glyph_cache.lock();
                prune_glyph_cache(&mut cache, &active_glyph_keys);
                persist_active_glyph_plans(&mut cache, &glyph_layout_cache, &active_glyph_keys);
            }
        },
    )
    .size_full()
}

#[cfg(feature = "ghostty_vt")]
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
                    fg: ghostty_vt::Rgb { r: 0xAA, g: 0x00, b: 0x00 },
                    bg: ghostty_vt::Rgb { r: 0x00, g: 0x00, b: 0x00 },
                    flags: GHOSTTY_FLAG_BOLD,
                },
                ghostty_vt::StyleRun {
                    start_col: 3,
                    end_col: 3,
                    fg: ghostty_vt::Rgb { r: 0x00, g: 0xAA, b: 0x00 },
                    bg: ghostty_vt::Rgb { r: 0x00, g: 0x00, b: 0x00 },
                    flags: 0,
                },
            ],
            cells: Vec::new(),
            glyph_instances: Vec::new(),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let cells = grid_cells_from_row(&row, 10);
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
                fg: ghostty_vt::Rgb { r: 0xFF, g: 0x88, b: 0x33 },
                bg: ghostty_vt::Rgb { r: 0x11, g: 0x22, b: 0x33 },
                flags: 0,
            }],
            cells: Vec::new(),
            glyph_instances: Vec::new(),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let cells = grid_cells_from_row(&row, 10);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].kind, GridCellKind::GeometricBlock);
        assert_eq!(cells[1].kind, GridCellKind::Text);
    }

    #[::core::prelude::v1::test]
    fn grid_cells_from_parts_matches_row_wrapper() {
        let row = CachedTerminalRow {
            text: SharedString::from("A🔥"),
            style_runs: vec![ghostty_vt::StyleRun {
                start_col: 1,
                end_col: 3,
                fg: ghostty_vt::Rgb { r: 0xAA, g: 0xBB, b: 0xCC },
                bg: ghostty_vt::Rgb { r: 0x11, g: 0x22, b: 0x33 },
                flags: GHOSTTY_FLAG_UNDERLINE,
            }],
            cells: Vec::new(),
            glyph_instances: Vec::new(),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        assert_eq!(
            grid_cells_from_parts(row.text.as_ref(), &row.style_runs, 10),
            grid_cells_from_row(&row, 10)
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

        assert_eq!(glyph_key_from_cell(&mono).render_path, GlyphRenderPath::AtlasMonochrome);
        assert_eq!(glyph_key_from_cell(&emoji).render_path, GlyphRenderPath::AtlasPolychrome);
        assert_eq!(glyph_key_from_cell(&block).render_path, GlyphRenderPath::Geometry);
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
        assert_eq!(instances[0].key.render_path, GlyphRenderPath::AtlasPolychrome);
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
        persist_active_glyph_plans(&mut shared, &local, &active_keys);

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
            fg: ghostty_vt::Rgb { r: 0xAA, g: 0xAA, b: 0xAA },
            bg: ghostty_vt::Rgb { r: 0x22, g: 0x44, b: 0x66 },
            flags: 0,
        }];

        assert_eq!(
            damage_spans_from_style_runs(&previous, &next, 6),
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
            fg: ghostty_vt::Rgb { r: 0xAA, g: 0xAA, b: 0xAA },
            bg: ghostty_vt::Rgb { r: 0x22, g: 0x44, b: 0x66 },
            flags: 0,
        }];

        assert_eq!(
            damage_spans_from_terminal_row(
                &previous_cells,
                &previous_styles,
                &next_cells,
                &next_styles,
                6,
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
