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
    GlyphCache, GridRendererConfig, SelectionPoint, TerminalSnapshot, glyph_requires_gpui_overlay,
};
#[cfg(target_os = "windows")]
use super::native_gpu_presenter::NativeGpuPresenter;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CursorOverlay {
    pub row: u16,
    pub col: u16,
    pub width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackedRefresh {
    Unchanged,
    Full,
    Partial,
}

/// GPU rendering backend — Vulkan preferred, DX12 fallback.
enum GpuBackend {
    Vulkan(ghostty_vt::VulkanRenderer),
    Dx12(ghostty_vt::GpuRenderer),
}

/// Wraps the GPU renderer with frame-to-frame image caching.
pub(super) struct GpuTerminalState {
    backend: GpuBackend,
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
    last_cursor: Option<CursorOverlay>,
    last_selection: Option<(SelectionPoint, SelectionPoint)>,
    #[cfg(target_os = "windows")]
    native_presenter: Option<NativeGpuPresenter>,
}

impl GpuTerminalState {
    /// Try to create a GPU renderer. Tries Vulkan first, then DX12.
    /// Returns None if both fail.
    pub fn new(width: u32, height: u32, font_size: f32) -> Option<Self> {
        let backend = if let Ok(vk) = ghostty_vt::VulkanRenderer::new(width, height, font_size) {
            log::info!(
                "Vulkan GPU renderer initialized: {}x{} font_size={:.1}",
                width, height, font_size
            );
            GpuBackend::Vulkan(vk)
        } else if let Ok(dx) = ghostty_vt::GpuRenderer::new(width, height, font_size) {
            log::info!(
                "DX12 GPU renderer initialized (Vulkan unavailable): {}x{} font_size={:.1}",
                width, height, font_size
            );
            GpuBackend::Dx12(dx)
        } else {
            log::warn!("No GPU renderer available (Vulkan and DX12 both failed)");
            return None;
        };
        Some(Self {
            backend,
            packed_rows: Vec::new(),
            packed_cells: Vec::new(),
            packed_row_offsets: Vec::new(),
            last_packed_revision: u64::MAX,
            frame_cells: Vec::new(),
            last_cursor: None,
            last_selection: None,
            #[cfg(target_os = "windows")]
            native_presenter: None,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        let ok = match &mut self.backend {
            GpuBackend::Vulkan(vk) => vk.resize(width, height),
            GpuBackend::Dx12(dx) => dx.resize(width, height),
        };
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
    ) -> PackedRefresh {
        if self.last_packed_revision != snapshot.content_revision {
            // If damaged_rows is empty but revision changed, no specific rows
            // reported damage — skip the expensive full rebuild and just update
            // the revision tracker.  Row-count changes and first-paint (u64::MAX)
            // are still caught explicitly.
            let needs_full_rebuild = self.packed_rows.len() != snapshot.rows.len()
                || self.last_packed_revision == u64::MAX;
            if !needs_full_rebuild && snapshot.damaged_rows.is_empty() {
                self.last_packed_revision = snapshot.content_revision;
                return PackedRefresh::Unchanged;
            }

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
                return PackedRefresh::Full;
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
                    return PackedRefresh::Partial;
                } else {
                    rebuild_packed_cells_from_rows(
                        &self.packed_rows,
                        &mut self.packed_cells,
                        &mut self.packed_row_offsets,
                    );
                    self.last_packed_revision = snapshot.content_revision;
                    return PackedRefresh::Full;
                }
            }
        }
        PackedRefresh::Unchanged
    }

    #[cfg(target_os = "windows")]
    pub fn hide_native_presenter(&self) {
        if let Some(presenter) = self.native_presenter.as_ref() {
            presenter.hide();
        }
    }

    fn prepare_frame_cells(
        &mut self,
        snapshot: &TerminalSnapshot,
        refresh: PackedRefresh,
        row_count: usize,
        term_cols: u16,
        cursor: Option<CursorOverlay>,
        selection: Option<(SelectionPoint, SelectionPoint)>,
    ) -> bool {
        if cursor.is_none()
            && selection.is_none()
            && self.last_cursor.is_none()
            && self.last_selection.is_none()
        {
            self.frame_cells.clear();
            return false;
        }

        let needs_full_copy = matches!(refresh, PackedRefresh::Full)
            || self.frame_cells.len() != self.packed_cells.len();
        if needs_full_copy {
            // Reuse allocation — resize + copy_from_slice avoids clear+extend overhead.
            self.frame_cells.resize(self.packed_cells.len(), ghostty_vt::GpuCellData {
                col: 0, row: 0, codepoint: 0, fg_rgba: 0, bg_rgba: 0, flags: 0, _pad: 0,
            });
            self.frame_cells.copy_from_slice(&self.packed_cells);
        } else {
            patch_packed_cells_from_rows(
                &self.packed_rows,
                &snapshot.damaged_rows,
                &self.packed_row_offsets,
                &mut self.frame_cells,
            );
            let stale_overlay_rects =
                overlay_damage_rects(term_cols, self.last_cursor, self.last_selection, None, None);
            restore_damage_rects(
                &self.packed_cells,
                &mut self.frame_cells,
                &self.packed_row_offsets,
                &stale_overlay_rects,
            );
        }

        paint_selection_overlay(
            &mut self.frame_cells,
            &self.packed_row_offsets,
            row_count,
            term_cols,
            selection,
        );
        paint_cursor_overlay(
            &mut self.frame_cells,
            &self.packed_row_offsets,
            term_cols,
            cursor,
        );

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
        let (cur_w, cur_h) = match &self.backend {
            GpuBackend::Vulkan(vk) => (vk.width(), vk.height()),
            GpuBackend::Dx12(dx) => (dx.width(), dx.height()),
        };
        if cur_w != target_w || cur_h != target_h {
            if !self.resize(target_w, target_h) {
                return false;
            }
        }

        let refresh =
            self.refresh_packed_cells(snapshot, config.term_cols, config.fg_color, config.bg_color);
        if self.packed_cells.is_empty() {
            return false;
        }
        let row_count = snapshot.rows.len();
        let use_frame_cells = self.prepare_frame_cells(
            snapshot,
            refresh,
            row_count,
            config.term_cols,
            cursor,
            selection,
        );
        let damage_rects = if matches!(refresh, PackedRefresh::Full) {
            Vec::new()
        } else {
            compute_damage_rects(
                snapshot,
                config.term_cols,
                cursor,
                self.last_cursor,
                selection,
                self.last_selection,
            )
        };
        let dirty_cells = if matches!(refresh, PackedRefresh::Full) {
            Vec::new()
        } else {
            compute_dirty_cells_from_rects(&damage_rects, config.term_cols)
        };

        // Native presentation requires DX12 (Vulkan swapchain not yet implemented)
        if self.native_presenter.is_none() {
            if let GpuBackend::Dx12(ref dx) = self.backend {
                self.native_presenter = NativeGpuPresenter::new(
                    parent_hwnd,
                    bounds,
                    dx.device_ptr(),
                    dx.command_queue_ptr(),
                )
                .ok();
            }
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
        if matches!(refresh, PackedRefresh::Unchanged) && damage_rects.is_empty() {
            self.last_cursor = cursor;
            self.last_selection = selection;
            return true;
        }
        let Some(back_buffer_ptr) = presenter.current_back_buffer_ptr() else {
            return false;
        };
        let rendered = match &mut self.backend {
            GpuBackend::Dx12(dx) => {
                if matches!(refresh, PackedRefresh::Full) {
                    dx.render_to_surface(
                        back_buffer_ptr,
                        cells,
                        config.term_cols as u32,
                        config.cell_width,
                        config.cell_height,
                    )
                } else {
                    dx.render_to_surface_delta_cells(
                        back_buffer_ptr,
                        cells,
                        &dirty_cells,
                        &damage_rects,
                        config.term_cols as u32,
                        config.cell_width,
                        config.cell_height,
                    )
                }
            }
            GpuBackend::Vulkan(_) => {
                // Vulkan swapchain (P1-C) not yet implemented — fall back
                return false;
            }
        };
        if !rendered {
            return false;
        }
        self.last_cursor = cursor;
        self.last_selection = selection;
        presenter.present().is_ok()
    }
}

pub(super) fn snapshot_can_present_natively(snapshot: &TerminalSnapshot) -> bool {
    // Fast path: only check damaged rows instead of full O(rows×cols) scan.
    // If no rows are damaged, the previous result is still valid — assume native.
    if snapshot.damaged_rows.is_empty() {
        return true;
    }
    for &row_idx in &snapshot.damaged_rows {
        if let Some(row) = snapshot.rows.get(row_idx as usize) {
            if row.cells.iter().any(|cell| glyph_requires_gpui_overlay(&cell.glyph)) {
                return false;
            }
        }
    }
    true
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

fn row_to_gpu_cells(
    row: &super::grid_renderer::CachedTerminalRow,
    row_idx: u16,
    term_cols: u16,
    default_fg: u32,
    default_bg: u32,
) -> Vec<ghostty_vt::GpuCellData> {
    // SIMD: bulk-initialize row with default colors (single memset + col fixup)
    let mut packed = Vec::with_capacity(term_cols as usize);
    super::simd_ops::init_default_row(&mut packed, row_idx, term_cols, default_fg, default_bg);

    // SIMD: batch-apply style run colors (fg/bg/flags per run range)
    if !row.style_runs.is_empty() {
        super::simd_ops::apply_style_run_colors(&mut packed, &row.style_runs);
    }

    // Apply cell overrides (codepoint, per-cell colors)
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

// Legacy row_to_gpu_cells inner loop preserved for reference:
#[allow(dead_code)]
fn row_to_gpu_cells_scalar(
    row: &super::grid_renderer::CachedTerminalRow,
    row_idx: u16,
    term_cols: u16,
    default_fg: u32,
    default_bg: u32,
) -> Vec<ghostty_vt::GpuCellData> {
    let mut packed = Vec::with_capacity(term_cols as usize);
    let runs = &row.style_runs;
    let mut run_idx: usize = 0;
    for col in 0..term_cols {
        let col1 = col + 1;
        while run_idx < runs.len() && runs[run_idx].end_col < col1 {
            run_idx += 1;
        }
        let (fg_rgb, bg_rgb, flags) = if run_idx < runs.len()
            && runs[run_idx].start_col <= col1
            && col1 <= runs[run_idx].end_col
        {
            let run = &runs[run_idx];
            let fg = ((run.fg.r as u32) << 16) | ((run.fg.g as u32) << 8) | (run.fg.b as u32);
            let bg = ((run.bg.r as u32) << 16) | ((run.bg.g as u32) << 8) | (run.bg.b as u32);
            (fg, bg, run.flags)
        } else {
            (default_fg, default_bg, 0)
        };
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

fn restore_damage_rects(
    packed_cells: &[ghostty_vt::GpuCellData],
    frame_cells: &mut [ghostty_vt::GpuCellData],
    packed_row_offsets: &[usize],
    rects: &[ghostty_vt::GpuDamageRect],
) {
    for rect in rects {
        for row in rect.row_start..rect.row_start + rect.row_count {
            let Some(&offset) = packed_row_offsets.get(row as usize) else {
                continue;
            };
            let start = offset + rect.start_col as usize;
            let end = start + rect.col_count as usize;
            if end <= packed_cells.len() && end <= frame_cells.len() {
                frame_cells[start..end].copy_from_slice(&packed_cells[start..end]);
            }
        }
    }
}

fn paint_selection_overlay(
    frame_cells: &mut [ghostty_vt::GpuCellData],
    packed_row_offsets: &[usize],
    row_count: usize,
    term_cols: u16,
    selection: Option<(SelectionPoint, SelectionPoint)>,
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
                term_cols
            };
            if sc >= ec {
                continue;
            }
            if let Some(&offset) = packed_row_offsets.get(row as usize) {
                for col in sc..ec {
                    let index = offset + col as usize;
                    if let Some(cell) = frame_cells.get_mut(index) {
                        cell.bg_rgba = 0xFF2F_6FED;
                    }
                }
            }
        }
    }
}

fn paint_cursor_overlay(
    frame_cells: &mut [ghostty_vt::GpuCellData],
    packed_row_offsets: &[usize],
    term_cols: u16,
    cursor: Option<CursorOverlay>,
) {
    if let Some(cursor) = cursor {
        if let Some(&offset) = packed_row_offsets.get(cursor.row as usize) {
            let width = cursor.width.max(1.0).round() as u16;
            let end_col = (cursor.col + width).min(term_cols);
            for col in cursor.col..end_col {
                let index = offset + col as usize;
                if let Some(cell) = frame_cells.get_mut(index) {
                    cell.bg_rgba = 0xFFF5_F5F7;
                    cell.fg_rgba = 0xFF00_0000;
                }
            }
        }
    }
}

fn push_rect(
    rects: &mut Vec<ghostty_vt::GpuDamageRect>,
    row_start: u32,
    row_count: u32,
    start_col: u32,
    col_count: u32,
) {
    if row_count == 0 || col_count == 0 {
        return;
    }
    rects.push(ghostty_vt::GpuDamageRect {
        start_col,
        col_count,
        row_start,
        row_count,
    });
}

fn append_selection_rects(
    rects: &mut Vec<ghostty_vt::GpuDamageRect>,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    term_cols: u16,
) {
    let Some((sel_start, sel_end)) = selection else {
        return;
    };
    for row in sel_start.row..=sel_end.row {
        let start_col = if row == sel_start.row {
            sel_start.col
        } else {
            0
        };
        let end_col = if row == sel_end.row {
            sel_end.col
        } else {
            term_cols
        };
        if start_col >= end_col {
            continue;
        }
        push_rect(
            rects,
            row as u32,
            1,
            start_col as u32,
            (end_col - start_col) as u32,
        );
    }
}

fn append_cursor_rect(
    rects: &mut Vec<ghostty_vt::GpuDamageRect>,
    cursor: Option<CursorOverlay>,
    term_cols: u16,
) {
    let Some(cursor) = cursor else {
        return;
    };
    let width = cursor.width.max(1.0).round() as u16;
    let end_col = (cursor.col + width).min(term_cols);
    if cursor.col >= end_col {
        return;
    }
    push_rect(
        rects,
        cursor.row as u32,
        1,
        cursor.col as u32,
        (end_col - cursor.col) as u32,
    );
}

fn merge_damage_rects(
    mut rects: Vec<ghostty_vt::GpuDamageRect>,
    term_cols: u16,
) -> Vec<ghostty_vt::GpuDamageRect> {
    rects.retain(|rect| {
        rect.col_count != 0 && rect.row_count != 0 && rect.start_col < term_cols as u32
    });
    // Short-circuit: 0 or 1 rects cannot merge, skip sorting entirely
    if rects.len() <= 1 {
        return rects;
    }
    rects.sort_unstable_by_key(|rect| {
        (
            rect.row_start,
            rect.row_count,
            rect.start_col,
            rect.col_count,
        )
    });

    let mut row_merged: Vec<ghostty_vt::GpuDamageRect> = Vec::with_capacity(rects.len());
    for rect in rects {
        if let Some(last) = row_merged.last_mut() {
            let last_col_end = last.start_col + last.col_count;
            let rect_col_end = rect.start_col + rect.col_count;
            if last.row_start == rect.row_start
                && last.row_count == rect.row_count
                && rect.start_col <= last_col_end
            {
                last.col_count = last
                    .col_count
                    .max(rect_col_end.saturating_sub(last.start_col));
                continue;
            }
        }
        row_merged.push(rect);
    }

    row_merged.sort_unstable_by_key(|rect| (rect.start_col, rect.col_count, rect.row_start));
    let mut merged: Vec<ghostty_vt::GpuDamageRect> = Vec::with_capacity(row_merged.len());
    for rect in row_merged {
        if let Some(last) = merged.last_mut() {
            let last_row_end = last.row_start + last.row_count;
            if last.start_col == rect.start_col
                && last.col_count == rect.col_count
                && rect.row_start <= last_row_end
            {
                let rect_end = rect.row_start + rect.row_count;
                last.row_count = last.row_count.max(rect_end.saturating_sub(last.row_start));
                continue;
            }
        }
        merged.push(rect);
    }

    merged
}

fn compute_damage_rects(
    snapshot: &TerminalSnapshot,
    term_cols: u16,
    cursor: Option<CursorOverlay>,
    previous_cursor: Option<CursorOverlay>,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    previous_selection: Option<(SelectionPoint, SelectionPoint)>,
) -> Vec<ghostty_vt::GpuDamageRect> {
    let mut rects = Vec::new();

    for &row_idx in &snapshot.damaged_rows {
        let Some(row) = snapshot.rows.get(row_idx as usize) else {
            continue;
        };
        if row.damage_spans.is_empty() {
            push_rect(&mut rects, row_idx as u32, 1, 0, term_cols as u32);
            continue;
        }
        for span in &row.damage_spans {
            if span.start_col >= span.end_col {
                continue;
            }
            push_rect(
                &mut rects,
                row_idx as u32,
                1,
                span.start_col as u32,
                (span.end_col - span.start_col) as u32,
            );
        }
    }

    rects.extend(overlay_damage_rects(
        term_cols,
        cursor,
        selection,
        previous_cursor,
        previous_selection,
    ));

    merge_damage_rects(rects, term_cols)
}

/// Compute sorted, deduplicated dirty cell indices from damage rects.
/// SIMD-optimized: uses bitmap approach (fits in L1 cache) instead of sort+dedup.
/// For a 200×50 grid, bitmap = 1.5 KB vs sort+dedup on potentially 10K entries.
fn compute_dirty_cells_from_rects(
    rects: &[ghostty_vt::GpuDamageRect],
    term_cols: u16,
) -> Vec<ghostty_vt::GpuDirtyCell> {
    if term_cols == 0 || rects.is_empty() {
        return Vec::new();
    }
    // Determine max row from rects to size the bitmap
    let max_row = rects
        .iter()
        .map(|r| r.row_start + r.row_count)
        .max()
        .unwrap_or(0);
    super::simd_ops::dirty_cells_from_bitmap(rects, term_cols, max_row)
}

fn overlay_damage_rects(
    term_cols: u16,
    cursor: Option<CursorOverlay>,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    previous_cursor: Option<CursorOverlay>,
    previous_selection: Option<(SelectionPoint, SelectionPoint)>,
) -> Vec<ghostty_vt::GpuDamageRect> {
    let mut rects = Vec::new();
    append_cursor_rect(&mut rects, previous_cursor, term_cols);
    append_cursor_rect(&mut rects, cursor, term_cols);
    append_selection_rects(&mut rects, previous_selection, term_cols);
    append_selection_rects(&mut rects, selection, term_cols);
    merge_damage_rects(rects, term_cols)
}

/// Create a GPUI Canvas element that renders the terminal via DX12 GPU pipeline.
///
/// This keeps GPUI alive for layout/input while the pixels are presented by a native DXGI swapchain.
pub(super) fn gpu_terminal_canvas(
    snapshot: Arc<TerminalSnapshot>,
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
        CursorOverlay, compute_damage_rects, compute_dirty_cells_from_rects,
        patch_packed_cells_from_rows, rebuild_packed_cells_from_rows, row_to_gpu_cells,
        snapshot_can_present_natively,
    };
    use crate::terminal::grid_renderer::SelectionPoint;
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
    fn row_to_gpu_cells_preserves_cjk_codepoint() {
        let row = CachedTerminalRow {
            text: SharedString::from("漢"),
            style_runs: Vec::new(),
            cells: vec![GridCell {
                col: 1,
                width: 2,
                glyph: "漢".into(),
                fg_rgb: 0x223344,
                bg_rgb: 0x556677,
                flags: 0,
                kind: GridCellKind::Text,
            }],
            glyph_instances: Vec::new(),
            damage_spans: vec![DamageSpan {
                start_col: 1,
                end_col: 3,
            }],
            damaged_glyph_instances: Vec::new(),
        };

        let packed = row_to_gpu_cells(&row, 4, 4, 0xAABBCC, 0x112233);
        assert_eq!(packed[1].codepoint, '漢' as u32);
        assert_eq!(packed[2].codepoint, 0);
        assert_eq!(packed[1].fg_rgba, 0xFF223344);
        assert_eq!(packed[2].bg_rgba, 0xFF556677);
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
    fn compute_damage_rects_merges_adjacent_rows_with_same_columns() {
        let mut snapshot = TerminalSnapshot::new(3);
        snapshot.damaged_rows = vec![0, 1, 2];
        snapshot.rows[0].damage_spans = vec![DamageSpan {
            start_col: 2,
            end_col: 5,
        }];
        snapshot.rows[1].damage_spans = vec![DamageSpan {
            start_col: 2,
            end_col: 5,
        }];
        snapshot.rows[2].damage_spans = vec![DamageSpan {
            start_col: 6,
            end_col: 8,
        }];

        let rects = compute_damage_rects(&snapshot, 10, None, None, None, None);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].start_col, 2);
        assert_eq!(rects[0].col_count, 3);
        assert_eq!(rects[0].row_start, 0);
        assert_eq!(rects[0].row_count, 2);
        assert_eq!(rects[1].start_col, 6);
        assert_eq!(rects[1].row_start, 2);
    }

    #[test]
    fn compute_damage_rects_merges_adjacent_spans_on_same_row_before_vertical_merge() {
        let mut snapshot = TerminalSnapshot::new(2);
        snapshot.damaged_rows = vec![0, 1];
        snapshot.rows[0].damage_spans = vec![
            DamageSpan {
                start_col: 2,
                end_col: 4,
            },
            DamageSpan {
                start_col: 4,
                end_col: 6,
            },
        ];
        snapshot.rows[1].damage_spans = vec![DamageSpan {
            start_col: 2,
            end_col: 6,
        }];

        let rects = compute_damage_rects(&snapshot, 10, None, None, None, None);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].start_col, 2);
        assert_eq!(rects[0].col_count, 4);
        assert_eq!(rects[0].row_start, 0);
        assert_eq!(rects[0].row_count, 2);
    }

    #[test]
    fn compute_dirty_cells_from_rects_sorts_and_deduplicates_overlap() {
        let rects = vec![
            ghostty_vt::GpuDamageRect {
                start_col: 1,
                col_count: 3,
                row_start: 2,
                row_count: 1,
            },
            ghostty_vt::GpuDamageRect {
                start_col: 3,
                col_count: 2,
                row_start: 2,
                row_count: 1,
            },
        ];

        let cells = compute_dirty_cells_from_rects(&rects, 8);
        let indices: Vec<u32> = cells.into_iter().map(|cell| cell.instance_index).collect();
        assert_eq!(indices, vec![17, 18, 19, 20]);
    }

    #[test]
    fn compute_dirty_cells_from_rects_expands_scroll_like_full_rows() {
        let rects = vec![ghostty_vt::GpuDamageRect {
            start_col: 0,
            col_count: 6,
            row_start: 3,
            row_count: 2,
        }];

        let cells = compute_dirty_cells_from_rects(&rects, 6);
        let indices: Vec<u32> = cells.into_iter().map(|cell| cell.instance_index).collect();
        assert_eq!(
            indices,
            vec![18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29]
        );
    }

    #[test]
    fn compute_damage_rects_includes_cursor_and_selection_changes() {
        let snapshot = TerminalSnapshot::new(4);

        let rects = compute_damage_rects(
            &snapshot,
            12,
            Some(CursorOverlay {
                row: 1,
                col: 3,
                width: 2.0,
            }),
            Some(CursorOverlay {
                row: 0,
                col: 1,
                width: 1.0,
            }),
            Some((
                SelectionPoint { row: 2, col: 4 },
                SelectionPoint { row: 3, col: 7 },
            )),
            None,
        );

        assert!(rects.iter().any(|rect| {
            rect.row_start == 0 && rect.row_count == 1 && rect.start_col == 1 && rect.col_count == 1
        }));
        assert!(rects.iter().any(|rect| {
            rect.row_start == 1 && rect.row_count == 1 && rect.start_col == 3 && rect.col_count == 2
        }));
        assert!(rects.iter().any(|rect| {
            rect.row_start == 2 && rect.row_count == 1 && rect.start_col == 4 && rect.col_count == 8
        }));
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
