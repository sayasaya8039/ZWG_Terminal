//! GPU-accelerated terminal rendering — bypasses GPUI's text shaping pipeline.
//!
//! Flow: terminal cells → GpuCellData → DX12 instanced draw.
//! Windows native path presents directly to a DXGI swapchain backbuffer.
//! Non-native frames fall back to GPUI text shaping instead of RenderImage readback.
//!
//! This module provides an alternative rendering path that replaces GPUI's per-glyph
//! text shaping with a single DX12 instanced draw call for the entire terminal grid.

use std::sync::Arc;

use gpui::*;
use parking_lot::Mutex;
#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HWND;

use super::grid_renderer::{
    glyph_requires_gpui_overlay, paint_geometry_cell, GlyphCache, GridRendererConfig,
    SelectionPoint, TerminalSnapshot,
};
#[cfg(target_os = "windows")]
use super::native_gpu_presenter::NativeGpuPresenter;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CursorOverlay {
    pub row: u16,
    pub col: u16,
    pub width: f32,
}

/// Wraps the DX12 GpuRenderer with frame-to-frame image caching.
pub(super) struct GpuTerminalState {
    renderer: ghostty_vt::GpuRenderer,
    /// Cached per-row GPU cell payload.
    packed_rows: Vec<Vec<ghostty_vt::GpuCellData>>,
    /// Flattened payload passed to the renderer.
    packed_cells: Vec<ghostty_vt::GpuCellData>,
    /// Starting offset of each row inside `packed_cells`.
    packed_row_offsets: Vec<usize>,
    /// Content revision associated with `packed_rows` / `packed_cells`.
    last_packed_revision: u64,
    /// Temporary frame payload with cursor/selection overlays applied.
    frame_cells: Vec<ghostty_vt::GpuCellData>,
    #[cfg(target_os = "windows")]
    native_presenter: Option<NativeGpuPresenter>,
}

impl GpuTerminalState {
    /// Try to create a DX12 GPU renderer. Returns None if DX12 init fails
    /// (e.g., no compatible GPU, driver issues).
    pub fn new(width: u32, height: u32, font_size: f32) -> Option<Self> {
        let renderer = ghostty_vt::GpuRenderer::new(width, height, font_size).ok()?;
        log::info!(
            "DX12 GPU renderer initialized: {}x{} font_size={:.1}",
            width,
            height,
            font_size
        );
        Some(Self {
            renderer,
            packed_rows: Vec::new(),
            packed_cells: Vec::new(),
            packed_row_offsets: Vec::new(),
            last_packed_revision: u64::MAX,
            frame_cells: Vec::new(),
            #[cfg(target_os = "windows")]
            native_presenter: None,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        let ok = self.renderer.resize(width, height);
        if ok {
            #[cfg(target_os = "windows")]
            if let Some(presenter) = self.native_presenter.as_ref() {
                presenter.hide();
            }
        }
        ok
    }

    fn refresh_packed_cells(
        &mut self,
        snapshot: &TerminalSnapshot,
        term_cols: u16,
        default_fg: u32,
        default_bg: u32,
    ) {
        if self.last_packed_revision != snapshot.content_revision {
            let needs_full_rebuild = self.packed_rows.len() != snapshot.rows.len()
                || self.last_packed_revision == u64::MAX
                || snapshot.damaged_rows.is_empty();

            if needs_full_rebuild {
                self.packed_rows = snapshot
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(row_idx, row)| {
                        row_to_gpu_cells(row, row_idx as u16, term_cols, default_fg, default_bg)
                    })
                    .collect();
                rebuild_packed_cells_from_rows(
                    &self.packed_rows,
                    &mut self.packed_cells,
                    &mut self.packed_row_offsets,
                );
                self.last_packed_revision = snapshot.content_revision;
                return;
            } else {
                let mut can_patch_flattened = true;
                for &row_idx in &snapshot.damaged_rows {
                    let index = row_idx as usize;
                    if let Some(row) = snapshot.rows.get(index) {
                        if index >= self.packed_rows.len() {
                            self.packed_rows
                                .resize_with(index + 1, Vec::<ghostty_vt::GpuCellData>::new);
                            can_patch_flattened = false;
                        }
                        let next_row =
                            row_to_gpu_cells(row, row_idx, term_cols, default_fg, default_bg);
                        if self.packed_rows[index].len() != next_row.len() {
                            can_patch_flattened = false;
                        }
                        self.packed_rows[index] = next_row;
                    }
                }

                if can_patch_flattened {
                    patch_packed_cells_from_rows(
                        &self.packed_rows,
                        &snapshot.damaged_rows,
                        &self.packed_row_offsets,
                        &mut self.packed_cells,
                    );
                    self.last_packed_revision = snapshot.content_revision;
                    return;
                } else {
                    rebuild_packed_cells_from_rows(
                        &self.packed_rows,
                        &mut self.packed_cells,
                        &mut self.packed_row_offsets,
                    );
                    self.last_packed_revision = snapshot.content_revision;
                    return;
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    pub fn hide_native_presenter(&self) {
        if let Some(presenter) = self.native_presenter.as_ref() {
            presenter.hide();
        }
    }

    fn prepare_frame_cells(
        &mut self,
        row_count: usize,
        term_cols: u16,
        cursor: Option<CursorOverlay>,
        selection: Option<(SelectionPoint, SelectionPoint)>,
    ) -> bool {
        if cursor.is_none() && selection.is_none() {
            self.frame_cells.clear();
            return false;
        }

        self.frame_cells.clear();
        self.frame_cells.extend_from_slice(&self.packed_cells);

        if let Some((sel_start, sel_end)) = selection {
            let max_row = sel_end.row.min(row_count.saturating_sub(1) as u16);
            for row in sel_start.row..=max_row {
                let sc = if row == sel_start.row {
                    sel_start.col
                } else {
                    0
                };
                let ec = if row == sel_end.row {
                    sel_end.col
                } else {
                    term_cols
                };
                if sc >= ec {
                    continue;
                }
                if let Some(&offset) = self.packed_row_offsets.get(row as usize) {
                    for col in sc..ec {
                        let index = offset + col as usize;
                        if let Some(cell) = self.frame_cells.get_mut(index) {
                            cell.bg_rgba = 0xFF2F_6FED;
                        }
                    }
                }
            }
        }

        if let Some(cursor) = cursor {
            if let Some(&offset) = self.packed_row_offsets.get(cursor.row as usize) {
                let width = cursor.width.max(1.0).round() as usize;
                for dx in 0..width {
                    let index = offset + cursor.col as usize + dx;
                    if let Some(cell) = self.frame_cells.get_mut(index) {
                        cell.bg_rgba = 0xFFF5_F5F7;
                        cell.fg_rgba = 0xFF00_0000;
                    }
                }
            }
        }

        true
    }

    #[cfg(target_os = "windows")]
    fn present_native(
        &mut self,
        snapshot: &TerminalSnapshot,
        bounds: Bounds<Pixels>,
        window: &Window,
        cursor: Option<CursorOverlay>,
        selection: Option<(SelectionPoint, SelectionPoint)>,
        config: &GridRendererConfig,
    ) -> bool {
        if !snapshot_can_present_natively(snapshot) {
            if let Some(presenter) = self.native_presenter.as_ref() {
                presenter.hide();
            }
            return false;
        }

        let Ok(parent_hwnd) = window_hwnd(window) else {
            return false;
        };

        let target_w: f32 = bounds.size.width.into();
        let target_h: f32 = bounds.size.height.into();
        let target_w = target_w.max(1.0).ceil() as u32;
        let target_h = target_h.max(1.0).ceil() as u32;
        if self.renderer.width() != target_w || self.renderer.height() != target_h {
            if !self.resize(target_w, target_h) {
                return false;
            }
        }

        self.refresh_packed_cells(snapshot, config.term_cols, config.fg_color, config.bg_color);
        if self.packed_cells.is_empty() {
            return false;
        }
        let row_count = snapshot.rows.len();
        let use_frame_cells =
            self.prepare_frame_cells(row_count, config.term_cols, cursor, selection);

        if self.native_presenter.is_none() {
            self.native_presenter = NativeGpuPresenter::new(
                parent_hwnd,
                bounds,
                self.renderer.device_ptr(),
                self.renderer.command_queue_ptr(),
            )
            .ok();
        }

        let Some(presenter) = self.native_presenter.as_mut() else {
            return false;
        };
        let cells = if use_frame_cells {
            self.frame_cells.as_slice()
        } else {
            self.packed_cells.as_slice()
        };

        if presenter.sync_bounds(parent_hwnd, bounds).is_err() {
            return false;
        }
        let Some(back_buffer_ptr) = presenter.current_back_buffer_ptr() else {
            return false;
        };
        if !self.renderer.render_to_surface(
            back_buffer_ptr,
            cells,
            config.term_cols as u32,
            config.cell_width,
            config.cell_height,
        ) {
            return false;
        }
        presenter.present().is_ok()
    }
}

pub(super) fn snapshot_can_present_natively(snapshot: &TerminalSnapshot) -> bool {
    snapshot.rows.iter().all(|row| {
        row.cells
            .iter()
            .all(|cell| !glyph_requires_gpui_overlay(&cell.glyph))
    })
}

#[cfg(target_os = "windows")]
fn window_hwnd(window: &Window) -> anyhow::Result<HWND> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|err| anyhow::anyhow!("getting raw window handle: {err}"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(raw) => Ok(HWND(raw.hwnd.get() as *mut core::ffi::c_void)),
        _ => Err(anyhow::anyhow!("GPUI window is not a Win32 window")),
    }
}

fn paint_terminal_overlays(
    bounds: Bounds<Pixels>,
    row_count: usize,
    cursor: Option<CursorOverlay>,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    config: GridRendererConfig,
    window: &mut Window,
) {
    if let Some((sel_start, sel_end)) = selection {
        let max_row = sel_end.row.min(row_count.saturating_sub(1) as u16);
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
                            + px(config.horizontal_text_padding + sc as f32 * config.cell_width),
                        bounds.origin.y + px(row as f32 * config.cell_height),
                    ),
                    size(
                        px((ec - sc) as f32 * config.cell_width),
                        px(config.cell_height),
                    ),
                ),
                rgba(0x2F6FED55),
            ));
        }
    }

    if let Some(cursor) = cursor {
        if (cursor.row as usize) < row_count {
            window.paint_quad(fill(
                Bounds::new(
                    point(
                        bounds.origin.x
                            + px(config.horizontal_text_padding
                                + cursor.col as f32 * config.cell_width),
                        bounds.origin.y + px(cursor.row as f32 * config.cell_height),
                    ),
                    size(px(config.cell_width * cursor.width), px(config.cell_height)),
                ),
                rgba(0xF5F5F780),
            ));
        }
    }
}

fn paint_geometry_rows(
    snapshot: &TerminalSnapshot,
    bounds: Bounds<Pixels>,
    config: &GridRendererConfig,
    window: &mut Window,
) {
    for (row_idx, row) in snapshot.rows.iter().enumerate() {
        for cell in &row.cells {
            if cell.kind == super::grid_renderer::GridCellKind::GeometricBlock {
                paint_geometry_cell(row_idx as u16, cell, bounds, config, window);
            }
        }
    }
}

fn row_to_gpu_cells(
    row: &super::grid_renderer::CachedTerminalRow,
    row_idx: u16,
    term_cols: u16,
    default_fg: u32,
    default_bg: u32,
) -> Vec<ghostty_vt::GpuCellData> {
    let mut packed = Vec::with_capacity(term_cols as usize);
    for col in 0..term_cols {
        let (fg_rgb, bg_rgb, flags) = super::grid_renderer::grid_cell_style_at(
            &row.style_runs,
            col + 1,
            default_fg,
            default_bg,
        );
        packed.push(ghostty_vt::GpuCellData {
            col,
            row: row_idx,
            codepoint: 0,
            fg_rgba: 0xFF00_0000 | fg_rgb,
            bg_rgba: 0xFF00_0000 | bg_rgb,
            flags: flags as u16,
            _pad: 0,
        });
    }

    for cell in &row.cells {
        let start = cell.col as usize;
        if start >= packed.len() {
            continue;
        }
        packed[start].codepoint = if glyph_requires_gpui_overlay(&cell.glyph) {
            0
        } else {
            cell.glyph.chars().next().unwrap_or(' ') as u32
        };
        packed[start].fg_rgba = 0xFF00_0000 | cell.fg_rgb;
        packed[start].bg_rgba = 0xFF00_0000 | cell.bg_rgb;
        packed[start].flags = cell.flags as u16;

        for offset in 1..cell.width as usize {
            let col = start + offset;
            if col >= packed.len() {
                break;
            }
            packed[col].fg_rgba = 0xFF00_0000 | cell.fg_rgb;
            packed[col].bg_rgba = 0xFF00_0000 | cell.bg_rgb;
            packed[col].flags = cell.flags as u16;
        }
    }

    packed
}

fn rebuild_packed_cells_from_rows(
    packed_rows: &[Vec<ghostty_vt::GpuCellData>],
    packed_cells: &mut Vec<ghostty_vt::GpuCellData>,
    packed_row_offsets: &mut Vec<usize>,
) {
    packed_row_offsets.clear();
    packed_row_offsets.reserve(packed_rows.len());

    let total_cells: usize = packed_rows.iter().map(Vec::len).sum();
    packed_cells.clear();
    packed_cells.reserve(total_cells);

    let mut offset = 0;
    for row in packed_rows {
        packed_row_offsets.push(offset);
        packed_cells.extend(row.iter().copied());
        offset += row.len();
    }
}

fn patch_packed_cells_from_rows(
    packed_rows: &[Vec<ghostty_vt::GpuCellData>],
    damaged_rows: &[u16],
    packed_row_offsets: &[usize],
    packed_cells: &mut [ghostty_vt::GpuCellData],
) {
    for &row_idx in damaged_rows {
        let index = row_idx as usize;
        let Some(row) = packed_rows.get(index) else {
            continue;
        };
        let Some(&offset) = packed_row_offsets.get(index) else {
            continue;
        };
        let end = offset + row.len();
        if end <= packed_cells.len() {
            packed_cells[offset..end].copy_from_slice(row);
        }
    }
}

/// Create a GPUI Canvas element that renders the terminal via DX12 GPU pipeline.
///
/// This keeps GPUI alive for layout/input while the pixels are presented by a native DXGI swapchain.
pub(super) fn gpu_terminal_canvas(
    snapshot: TerminalSnapshot,
    cursor: Option<CursorOverlay>,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    config: GridRendererConfig,
    gpu_state: Arc<Mutex<GpuTerminalState>>,
    glyph_cache: GlyphCache,
) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window: &mut Window, cx: &mut App| {
            #[cfg(target_os = "windows")]
            let native_presented = {
                let mut state = gpu_state.lock();
                state.present_native(&snapshot, bounds, window, cursor, selection, &config)
            };
            #[cfg(not(target_os = "windows"))]
            let native_presented = false;

            if native_presented {
                // Native presenter owns the pixels; GPUI only keeps layout and input alive.
            } else {
                // Fallback should be chosen by the caller; keep this path inert.
            }

            if !native_presented {
                let _ = cx;
                let _ = &glyph_cache;
            }
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use gpui::SharedString;

    use super::{
        patch_packed_cells_from_rows, rebuild_packed_cells_from_rows, row_to_gpu_cells,
        snapshot_can_present_natively,
    };
    use crate::terminal::grid_renderer::{
        CachedTerminalRow, DamageSpan, GridCell, GridCellKind, TerminalSnapshot,
    };

    #[test]
    fn row_to_gpu_cells_preserves_cell_payload() {
        let row = CachedTerminalRow {
            text: SharedString::from("A"),
            style_runs: Vec::new(),
            cells: vec![GridCell {
                col: 3,
                width: 1,
                glyph: "A".into(),
                fg_rgb: 0x112233,
                bg_rgb: 0x445566,
                flags: 0x08,
                kind: GridCellKind::Text,
            }],
            glyph_instances: Vec::new(),
            damage_spans: vec![DamageSpan {
                start_col: 3,
                end_col: 4,
            }],
            damaged_glyph_instances: Vec::new(),
        };

        let packed = row_to_gpu_cells(&row, 7, 8, 0xAABBCC, 0x112233);
        assert_eq!(packed.len(), 8);
        assert_eq!(packed[3].col, 3);
        assert_eq!(packed[3].row, 7);
        assert_eq!(packed[3].codepoint, 'A' as u32);
        assert_eq!(packed[3].fg_rgba, 0xFF112233);
        assert_eq!(packed[3].bg_rgba, 0xFF445566);
        assert_eq!(packed[3].flags, 0x08);
        assert_eq!(packed[0].codepoint, 0);
    }

    #[test]
    fn row_to_gpu_cells_preserves_geometry_codepoint() {
        let row = CachedTerminalRow {
            text: SharedString::from("─"),
            style_runs: Vec::new(),
            cells: vec![GridCell {
                col: 0,
                width: 1,
                glyph: "─".into(),
                fg_rgb: 0x112233,
                bg_rgb: 0x445566,
                flags: 0,
                kind: GridCellKind::GeometricBlock,
            }],
            glyph_instances: Vec::new(),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let packed = row_to_gpu_cells(&row, 2, 2, 0xAABBCC, 0x112233);
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].codepoint, '─' as u32);
    }

    #[test]
    fn rebuild_packed_cells_tracks_row_offsets() {
        let rows = vec![
            vec![ghostty_vt::GpuCellData {
                col: 0,
                row: 0,
                codepoint: 'A' as u32,
                fg_rgba: 1,
                bg_rgba: 2,
                flags: 0,
                _pad: 0,
            }],
            vec![
                ghostty_vt::GpuCellData {
                    col: 0,
                    row: 1,
                    codepoint: 'B' as u32,
                    fg_rgba: 3,
                    bg_rgba: 4,
                    flags: 0,
                    _pad: 0,
                },
                ghostty_vt::GpuCellData {
                    col: 1,
                    row: 1,
                    codepoint: 'C' as u32,
                    fg_rgba: 5,
                    bg_rgba: 6,
                    flags: 0,
                    _pad: 0,
                },
            ],
        ];
        let mut packed = Vec::new();
        let mut offsets = Vec::new();

        rebuild_packed_cells_from_rows(&rows, &mut packed, &mut offsets);

        assert_eq!(offsets, vec![0, 1]);
        assert_eq!(packed.len(), 3);
        assert_eq!(packed[2].codepoint, 'C' as u32);
    }

    #[test]
    fn patch_packed_cells_updates_only_changed_row_slice() {
        let mut rows = vec![
            vec![ghostty_vt::GpuCellData {
                col: 0,
                row: 0,
                codepoint: 'A' as u32,
                fg_rgba: 1,
                bg_rgba: 2,
                flags: 0,
                _pad: 0,
            }],
            vec![ghostty_vt::GpuCellData {
                col: 0,
                row: 1,
                codepoint: 'B' as u32,
                fg_rgba: 3,
                bg_rgba: 4,
                flags: 0,
                _pad: 0,
            }],
        ];
        let mut packed = Vec::new();
        let mut offsets = Vec::new();
        rebuild_packed_cells_from_rows(&rows, &mut packed, &mut offsets);

        rows[1][0].codepoint = 'Z' as u32;
        rows[1][0].fg_rgba = 9;
        patch_packed_cells_from_rows(&rows, &[1], &offsets, &mut packed);

        assert_eq!(packed[0].codepoint, 'A' as u32);
        assert_eq!(packed[1].codepoint, 'Z' as u32);
        assert_eq!(packed[1].fg_rgba, 9);
    }

    #[test]
    fn snapshot_native_presenter_accepts_geometry_cells() {
        let mut snapshot = TerminalSnapshot::new(1);
        snapshot.rows[0].cells.push(GridCell {
            col: 0,
            width: 1,
            glyph: "─".into(),
            fg_rgb: 0xFFFFFF,
            bg_rgb: 0x000000,
            flags: 0,
            kind: GridCellKind::GeometricBlock,
        });

        assert!(snapshot_can_present_natively(&snapshot));
    }

    #[test]
    fn snapshot_native_presenter_rejects_overlay_glyphs() {
        let mut snapshot = TerminalSnapshot::new(1);
        snapshot.rows[0].cells.push(GridCell {
            col: 0,
            width: 2,
            glyph: "🐷".into(),
            fg_rgb: 0xFFFFFF,
            bg_rgb: 0x000000,
            flags: 0,
            kind: GridCellKind::Text,
        });

        assert!(!snapshot_can_present_natively(&snapshot));
    }
}
