//! GPU-accelerated terminal rendering — bypasses GPUI's text shaping pipeline.
//!
//! Flow: terminal cells → GpuCellData → DX12 instanced draw → RGBA readback → GPUI paint_image
//!
//! This module provides an alternative rendering path that replaces GPUI's per-glyph
//! text shaping with a single DX12 instanced draw call for the entire terminal grid.

use std::sync::Arc;

use gpui::*;
use parking_lot::Mutex;

use super::grid_renderer::{GridRendererConfig, SelectionPoint, TerminalSnapshot};
use super::DEFAULT_BG;

/// Wraps the DX12 GpuRenderer with frame-to-frame image caching.
pub(super) struct GpuTerminalState {
    renderer: ghostty_vt::GpuRenderer,
    /// Cached image from last render — reused if snapshot unchanged
    last_image: Option<Arc<RenderImage>>,
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
            last_image: None,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        let ok = self.renderer.resize(width, height);
        if ok {
            self.last_image = None; // Invalidate cache on resize
        }
        ok
    }

    /// Render terminal cells via DX12 and produce a GPUI-compatible RenderImage.
    pub fn render_frame(
        &mut self,
        snapshot: &TerminalSnapshot,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<Arc<RenderImage>> {
        let cells = snapshot_to_gpu_cells(snapshot);
        if cells.is_empty() {
            return None;
        }

        // Query dimensions before render (render borrows self mutably)
        let width = self.renderer.width();
        let height = self.renderer.height();
        let stride = self.renderer.pixel_stride();
        let pixels = self.renderer.render(&cells, cell_width, cell_height)?;

        // Strip DX12 stride padding (256-byte aligned) → contiguous RGBA
        let row_bytes = (width * 4) as usize;
        let mut rgba = Vec::with_capacity(row_bytes * height as usize);
        for y in 0..height {
            let row_start = (y * stride) as usize;
            rgba.extend_from_slice(&pixels[row_start..row_start + row_bytes]);
        }

        let rgba_image = image::RgbaImage::from_raw(width, height, rgba)?;
        let frame = image::Frame::from_parts(
            rgba_image,
            0,
            0,
            image::Delay::from_numer_denom_ms(0, 1),
        );
        let render_image = Arc::new(RenderImage::new(vec![frame]));
        self.last_image = Some(render_image.clone());
        Some(render_image)
    }

    pub fn last_image(&self) -> Option<Arc<RenderImage>> {
        self.last_image.clone()
    }
}

/// Convert a terminal snapshot into the flat GpuCellData array expected by the DX12 renderer.
fn snapshot_to_gpu_cells(snapshot: &TerminalSnapshot) -> Vec<ghostty_vt::GpuCellData> {
    let mut data = Vec::with_capacity(
        snapshot
            .rows
            .iter()
            .map(|r| r.cells.len())
            .sum::<usize>(),
    );

    for (row_idx, row) in snapshot.rows.iter().enumerate() {
        for cell in &row.cells {
            let codepoint = cell.glyph.chars().next().unwrap_or(' ') as u32;
            // Pack RGB → ARGB (alpha=0xFF)
            let fg_rgba = 0xFF00_0000 | cell.fg_rgb;
            let bg_rgba = 0xFF00_0000 | cell.bg_rgb;

            data.push(ghostty_vt::GpuCellData {
                col: cell.col,
                row: row_idx as u16,
                codepoint,
                fg_rgba,
                bg_rgba,
                flags: cell.flags as u16,
                _pad: 0,
            });
        }
    }
    data
}

/// Create a GPUI Canvas element that renders the terminal via DX12 GPU pipeline.
///
/// This replaces `terminal_canvas()` from grid_renderer.rs with a single DX12
/// instanced draw call instead of per-glyph GPUI text shaping.
pub(super) fn gpu_terminal_canvas(
    snapshot: TerminalSnapshot,
    selection: Option<(SelectionPoint, SelectionPoint)>,
    config: GridRendererConfig,
    gpu_state: Arc<Mutex<GpuTerminalState>>,
) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _cx: &mut App| {
            let bounds_w: f32 = bounds.size.width.into();
            let bounds_h: f32 = bounds.size.height.into();

            // Render terminal via DX12
            let render_image = {
                let mut state = gpu_state.lock();

                // Ensure renderer matches current viewport
                let cur_w = state.renderer.width();
                let cur_h = state.renderer.height();
                let target_w = bounds_w.ceil() as u32;
                let target_h = bounds_h.ceil() as u32;
                if cur_w != target_w || cur_h != target_h {
                    state.resize(target_w, target_h);
                }

                state.render_frame(&snapshot, config.cell_width, config.cell_height)
            };

            if let Some(image) = render_image {
                let _ = window.paint_image(
                    bounds,
                    Corners::default(),
                    image,
                    0,
                    false,
                );
            } else {
                // Fallback: solid background if DX12 render failed
                window.paint_quad(fill(bounds, Hsla::from(rgb(DEFAULT_BG))));
            }

            // Selection overlay (rendered on top of DX12 output via GPUI)
            if let Some((sel_start, sel_end)) = selection {
                let num_rows = snapshot.rows.len();
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
                                    + px(
                                        config.horizontal_text_padding
                                            + sc as f32 * config.cell_width,
                                    ),
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

            // Cursor overlay (always rendered via GPUI for crisp edges)
            let num_rows = snapshot.rows.len();
            if (snapshot.cursor_y as usize) < num_rows {
                let cursor_col = snapshot.cursor_x.min(config.term_cols.saturating_sub(1));
                let cursor_width = snapshot
                    .rows
                    .get(snapshot.cursor_y as usize)
                    .and_then(|row| {
                        row.cells.iter().find(|c| {
                            c.col <= cursor_col && cursor_col < c.col + c.width as u16
                        })
                    })
                    .map(|c| c.width as f32)
                    .unwrap_or(1.0);

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
                        size(
                            px(config.cell_width * cursor_width),
                            px(config.cell_height),
                        ),
                    ),
                    rgba(0xF5F5F780),
                ));
            }
        },
    )
    .size_full()
}
