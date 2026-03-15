//! Terminal pane — GPUI view that renders the terminal and handles input

use std::ops::Range;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use gpui::ElementInputHandler;
use gpui::*;

use super::TerminalSettings;
#[cfg(feature = "ghostty_vt")]
use super::gpu_view::{
    CursorOverlay, GpuTerminalState, gpu_terminal_canvas, snapshot_can_present_natively,
};
#[cfg(feature = "ghostty_vt")]
use super::grid_renderer::resolve_cursor_cell;
use super::grid_renderer::{
    GlyphCache, GridRendererConfig, SelectionPoint, TerminalSnapshot, col_to_char_index,
    damage_spans_from_terminal_row, full_row_damage, glyph_instances_in_damage,
    grid_cells_from_parts, patch_cells_in_damage, patch_glyph_instances_in_damage, terminal_canvas,
};
use super::pty::{ConPtyConfig, spawn_pty};
use super::surface::TerminalSurface;
#[cfg(feature = "ghostty_vt")]
use parking_lot::Mutex;

const HORIZONTAL_TEXT_PADDING: f32 = 4.0;
const MAX_FRAME_COALESCE_MICROS: u64 = 1_667;
const ASYNC_PARSE_SETTLE_MILLIS: u64 = 2;
/// Fallback values — replaced at runtime by measured font metrics
const CELL_WIDTH_FALLBACK: f32 = 8.4;
const CELL_HEIGHT_FALLBACK: f32 = 19.5;
pub const CELL_WIDTH_ESTIMATE: f32 = CELL_WIDTH_FALLBACK;
pub const CELL_HEIGHT_ESTIMATE: f32 = CELL_HEIGHT_FALLBACK;
pub const WINDOW_CHROME_HEIGHT: f32 = 60.0;

#[cfg(feature = "ghostty_vt")]
#[derive(Clone)]
struct GhosttyRowUpdate {
    row: u16,
    text: String,
    style_runs: Vec<ghostty_vt::StyleRun>,
}

#[cfg(not(feature = "ghostty_vt"))]
#[derive(Clone)]
struct FallbackRowUpdate {
    row: u16,
    text: String,
    generation: u64,
}

#[cfg(feature = "ghostty_vt")]
fn apply_ghostty_row_update(
    cached_row: &mut super::grid_renderer::CachedTerminalRow,
    row_update: GhosttyRowUpdate,
    term_cols: u16,
    default_fg: u32,
    default_bg: u32,
    force_full: bool,
) -> bool {
    let next_cells = grid_cells_from_parts(
        &row_update.text,
        &row_update.style_runs,
        term_cols,
        default_fg,
        default_bg,
    );
    let damage_spans = if force_full {
        full_row_damage(term_cols)
    } else {
        damage_spans_from_terminal_row(
            &cached_row.cells,
            &cached_row.style_runs,
            &next_cells,
            &row_update.style_runs,
            term_cols,
            default_fg,
            default_bg,
        )
    };
    let cells = if force_full {
        next_cells.clone()
    } else {
        patch_cells_in_damage(&cached_row.cells, &next_cells, &damage_spans)
    };
    let glyph_instances = if force_full {
        super::grid_renderer::glyph_instances_from_cells(&cells, row_update.row)
    } else {
        patch_glyph_instances_in_damage(
            &cached_row.glyph_instances,
            &next_cells,
            row_update.row,
            &damage_spans,
        )
    };
    let damaged_glyph_instances = glyph_instances_in_damage(&cells, row_update.row, &damage_spans);
    let row_changed = !damage_spans.is_empty()
        || cached_row.text.as_ref() != row_update.text.as_str()
        || cached_row.style_runs != row_update.style_runs;

    cached_row.text = SharedString::from(row_update.text);
    cached_row.style_runs = row_update.style_runs;
    cached_row.cells = cells;
    cached_row.glyph_instances = glyph_instances;
    cached_row.damage_spans = damage_spans;
    cached_row.damaged_glyph_instances = damaged_glyph_instances;
    row_changed
}

/// Measure the actual monospace cell dimensions from the configured font.
/// Cell width = max advance width of representative monospace glyphs.
/// Cell height = ascent + descent (no extra leading — required for
/// box-drawing characters │─┌┐└┘ to connect between adjacent rows).
fn measure_cell_dimensions(cx: &App, font_family: &str, font_size_px: f32) -> (f32, f32) {
    let text_system = cx.text_system();
    let font_desc = font(SharedString::from(font_family.to_string()));
    let font_id = text_system.resolve_font(&font_desc);
    let font_size = px(font_size_px);

    let cell_width = ['M', 'W', '@', '0', '█', '│']
        .into_iter()
        .filter_map(|ch| text_system.advance(font_id, font_size, ch).ok())
        .map(|size| {
            let w: f32 = size.width.into();
            if w > 1.0 { w } else { CELL_WIDTH_FALLBACK }
        })
        .fold(CELL_WIDTH_FALLBACK, f32::max);

    let ascent: f32 = text_system.ascent(font_id, font_size).into();
    let descent: f32 = text_system.descent(font_id, font_size).into();
    // descent may be negative (OpenType convention) — use abs
    let cell_height = ascent + descent.abs();
    let cell_height = if cell_height > font_size_px {
        cell_height
    } else {
        CELL_HEIGHT_FALLBACK
    };

    // Use exact measured values — NO rounding.
    // cell_height = ascent + descent means paint_line's padding_top = 0,
    // so glyphs fill the full cell height with no gaps between rows.
    // Backgrounds are painted manually via paint_quad at grid positions.
    (cell_width, cell_height)
}

// Figma-aligned chrome colors for status text
const SUBTEXT0: u32 = 0x8E8E93;
const SURFACE0: u32 = 0x48484A;
const RED: u32 = 0xFF5F57;
const SELECTION_BG: u32 = 0x2F6FED;

// ── IME hook: fix Japanese/Chinese/Korean input for gpui 0.2.2 ──────
//
// gpui 0.2.2 calls TranslateMessage inside WndProc with a synthetic MSG
// (time=0), preventing IME from generating WM_IME_COMPOSITION.
// WH_GETMESSAGE hook intercepts VK_PROCESSKEY and calls TranslateMessage
// with the real MSG so IME composition works correctly.

static IME_VK_PROCESSKEY: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
unsafe extern "system" fn ime_getmessage_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, MSG, PM_REMOVE, TranslateMessage, WM_KEYDOWN,
    };

    if code >= 0 && wparam.0 == PM_REMOVE.0 as usize {
        unsafe {
            let msg = &*(lparam.0 as *const MSG);
            if msg.message == WM_KEYDOWN {
                let vk = (msg.wParam.0 & 0xFFFF) as u16;
                if vk == 0xE5 {
                    // VK_PROCESSKEY: IME is processing this key.
                    // Call TranslateMessage with the ORIGINAL MSG (real time/pt).
                    let _ = TranslateMessage(msg as *const MSG);
                    IME_VK_PROCESSKEY.store(true, Ordering::Release);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(target_os = "windows")]
fn install_ime_hook() {
    use std::sync::Once;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_GETMESSAGE};

    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let thread_id = GetCurrentThreadId();
        match SetWindowsHookExW(
            WH_GETMESSAGE,
            Some(ime_getmessage_hook_proc),
            None,
            thread_id,
        ) {
            Ok(_) => log::info!("IME GetMessage hook installed"),
            Err(e) => log::error!("Failed to install IME hook: {}", e),
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn install_ime_hook() {}

fn normalize_terminal_newlines(text: &str) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    normalized.into_bytes()
}

fn quote_path_for_terminal(path: &str) -> String {
    if path.contains([' ', '\t']) {
        format!("\"{}\"", path.replace('"', "\\\""))
    } else {
        path.to_string()
    }
}

fn format_dropped_paths(paths: &ExternalPaths) -> String {
    paths
        .paths()
        .iter()
        .map(|path| quote_path_for_terminal(&path.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn viewport_rows_to_refresh(
    rows: u16,
    force_full: bool,
    scroll_delta: i32,
    dirty_rows: Option<Vec<u16>>,
) -> Vec<u16> {
    if rows == 0 {
        return Vec::new();
    }

    if force_full || scroll_delta != 0 {
        return (0..rows).collect();
    }

    dirty_rows.unwrap_or_else(|| (0..rows).collect())
}

fn scroll_lines_from_wheel_delta(
    delta: ScrollDelta,
    cell_height: f32,
    line_remainder: &mut f32,
) -> i32 {
    let line_height = cell_height.max(1.0);
    let line_delta = match delta {
        ScrollDelta::Lines(delta) => delta.y,
        ScrollDelta::Pixels(delta) => {
            let pixels: f32 = delta.y.into();
            pixels / line_height
        }
    };

    // GPUI reports positive Y for wheel-up on Windows. Viewport scrolling uses
    // positive lines to move upward into scrollback, so invert once here.
    let total = *line_remainder - line_delta;
    let whole_lines = if total >= 0.0 {
        total.floor() as i32
    } else {
        total.ceil() as i32
    };
    *line_remainder = total - whole_lines as f32;
    whole_lines
}

fn terminal_layout_size(
    viewport_size: Size<Pixels>,
    last_bounds: Option<Bounds<Pixels>>,
) -> (f32, f32) {
    if let Some(bounds) = last_bounds {
        let width: f32 = bounds.size.width.into();
        let height: f32 = bounds.size.height.into();
        return (width.max(1.0), height.max(100.0));
    }

    let width: f32 = viewport_size.width.into();
    let height: f32 = viewport_size.height.into();
    (width.max(1.0), (height - WINDOW_CHROME_HEIGHT).max(100.0))
}

fn should_defer_keystroke_to_ime(ks: &Keystroke, ime_processkey_pending: bool) -> bool {
    if !ime_processkey_pending {
        return false;
    }

    !ks.key_char
        .as_ref()
        .is_some_and(|key_char| !key_char.is_empty())
}

/// Terminal connection state — two-phase init pattern
enum TerminalState {
    /// PTY is being spawned in background
    Pending,
    /// PTY is connected and running
    Running,
    /// PTY spawn failed
    Failed(String),
}

/// Terminal pane: GPUI component that wraps a TerminalSurface
pub struct TerminalPane {
    surface: TerminalSurface,
    focus_handle: FocusHandle,
    input_suppressed: Arc<AtomicBool>,
    state: TerminalState,
    snapshot: TerminalSnapshot,
    /// Cached cell dimensions
    cell_width: f32,
    cell_height: f32,
    font_family: SharedString,
    font_size: f32,
    cursor_blink: bool,
    copy_on_select: bool,
    gpu_acceleration: bool,
    fg_color: u32,
    bg_color: u32,
    background_image_path: Option<String>,
    background_image_opacity: f32,
    global_hotkeys: Vec<String>,
    blink_started_at: Instant,
    /// Current terminal size in cells
    term_cols: u16,
    term_rows: u16,
    /// Last known layout size for resize detection
    last_width: f32,
    last_height: f32,
    last_bounds: Option<Bounds<Pixels>>,
    selection_anchor: Option<SelectionPoint>,
    selection_head: Option<SelectionPoint>,
    is_selecting: bool,
    ime_composing: bool,
    wheel_scroll_line_remainder: f32,
    /// Keystrokes buffered while PTY is still connecting (Pending state)
    pending_input: Vec<u8>,
    pending_process_exit_status: Option<i32>,
    /// Cross-frame glyph layout cache — avoids reshaping unchanged glyphs every paint
    glyph_cache: GlyphCache,
    /// DX12 GPU renderer state — bypasses GPUI text shaping when available
    #[cfg(feature = "ghostty_vt")]
    gpu_state: Option<Arc<Mutex<GpuTerminalState>>>,
    #[cfg(not(feature = "ghostty_vt"))]
    row_generations: Vec<u64>,
}

impl TerminalPane {
    pub fn new(shell: &str, settings: TerminalSettings, cx: &mut Context<Self>) -> Self {
        // Install IME hook once per process
        install_ime_hook();

        let focus_handle = cx.focus_handle();
        let mut surface =
            TerminalSurface::new(settings.cols, settings.rows, settings.scrollback_lines);
        surface.set_default_colors(settings.fg_color, settings.bg_color);
        let event_rx = surface.take_event_rx();

        // Phase A: Return immediately with Pending state (<1ms)
        // Phase B: Spawn PTY in background thread
        let shell_owned = shell.to_string();
        let initial_cols = settings.cols;
        let initial_rows = settings.rows;
        cx.spawn(
            async move |this: WeakEntity<TerminalPane>, cx: &mut AsyncApp| {
                // Run ConPTY creation on background executor (off UI thread)
                let shell_for_spawn = shell_owned.clone();
                let pty_result = cx
                    .background_executor()
                    .spawn(async move {
                        let config = ConPtyConfig {
                            shell: shell_for_spawn,
                            cols: initial_cols,
                            rows: initial_rows,
                            working_directory: None,
                            env: Vec::new(),
                        };
                        spawn_pty(config)
                    })
                    .await;

                // Phase C: Attach PTY to surface on executor context
                let _ = this.update(cx, |pane: &mut TerminalPane, cx| {
                    match pty_result {
                        Ok(pty) => {
                            if let Err(e) = pane.surface.attach_pty(Arc::new(pty)) {
                                pane.state = TerminalState::Failed(e.to_string());
                                log::error!("Failed to attach PTY: {}", e);
                            } else {
                                pane.state = TerminalState::Running;
                                pane.refresh_snapshot(true);
                                log::info!("PTY connected for shell: {}", shell_owned);

                                // Flush any keystrokes buffered during Pending state
                                if !pane.pending_input.is_empty() {
                                    let buf = std::mem::take(&mut pane.pending_input);
                                    let _ = pane.surface.write_input(&buf);
                                }
                            }
                        }
                        Err(e) => {
                            pane.state = TerminalState::Failed(e.to_string());
                            log::error!("Failed to spawn shell: {}", e);
                        }
                    }
                    cx.notify();
                });
            },
        )
        .detach();

        // Wait for PTY output, then coalesce updates with an upper bound matching
        // a 600Hz frame budget. This keeps bursty PTY output from flooding the UI
        // while still allowing high refresh-rate panels to update promptly.
        cx.spawn(
            async move |this: WeakEntity<TerminalPane>, cx: &mut AsyncApp| {
                let frame_budget = std::time::Duration::from_micros(MAX_FRAME_COALESCE_MICROS);
                let mut last_presented: Option<std::time::Instant> = None;

                loop {
                    let Ok(event) = event_rx.recv_async().await else {
                        break;
                    };

                    let mut process_exit_status = match event {
                        super::surface::TerminalEvent::ProcessExited(code) => Some(code),
                        _ => None,
                    };
                    while let Ok(event) = event_rx.try_recv() {
                        if let super::surface::TerminalEvent::ProcessExited(code) = event {
                            process_exit_status = Some(code);
                        }
                    }

                    if let Some(last_presented_at) = last_presented {
                        let elapsed = last_presented_at.elapsed();
                        if elapsed < frame_budget {
                            cx.background_executor().timer(frame_budget - elapsed).await;
                        }
                    }

                    let mut should_notify = match this.update(cx, |pane: &mut TerminalPane, _cx| {
                        if process_exit_status.is_some() {
                            pane.pending_process_exit_status = process_exit_status;
                        }
                        pane.refresh_snapshot(false)
                    }) {
                        Ok(should_notify) => should_notify,
                        Err(_) => break,
                    };

                    if !should_notify {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(ASYNC_PARSE_SETTLE_MILLIS))
                            .await;
                        should_notify = match this.update(cx, |pane: &mut TerminalPane, _cx| {
                            pane.refresh_snapshot(false)
                        }) {
                            Ok(should_notify) => should_notify,
                            Err(_) => break,
                        };
                    }

                    if should_notify
                        && this
                            .update(cx, |_pane: &mut TerminalPane, cx| {
                                cx.notify();
                            })
                            .is_err()
                    {
                        break;
                    }
                    if should_notify {
                        last_presented = Some(std::time::Instant::now());
                    }
                }
            },
        )
        .detach();

        let font_family = SharedString::from(settings.font_family.clone());
        let (measured_w, measured_h) =
            measure_cell_dimensions(cx, font_family.as_ref(), settings.font_size);
        log::info!(
            "Terminal cell: width={:.2}px height={:.2}px (fallback w={:.1} h={:.1})",
            measured_w,
            measured_h,
            CELL_WIDTH_FALLBACK,
            CELL_HEIGHT_FALLBACK,
        );

        #[cfg(feature = "ghostty_vt")]
        {
            if settings.gpu_acceleration {
                let initial_w = (settings.cols as f32 * measured_w + HORIZONTAL_TEXT_PADDING * 2.0)
                    .ceil() as u32;
                let initial_h = (settings.rows as f32 * measured_h).ceil() as u32;
                let gpu_init_width = initial_w.max(64);
                let gpu_init_height = initial_h.max(64);
                let this = cx.entity().downgrade();
                cx.spawn(async move |_, cx: &mut AsyncApp| {
                    let gpu_result = cx
                        .background_executor()
                        .spawn(async move {
                            GpuTerminalState::new(
                                gpu_init_width,
                                gpu_init_height,
                                settings.font_size,
                            )
                        })
                        .await;

                    let _ = this.update(cx, |pane: &mut TerminalPane, cx| {
                        match gpu_result {
                            Some(state) => {
                                log::info!(
                                    "DX12 GPU terminal renderer active — bypassing GPUI text shaping"
                                );
                                pane.gpu_state = Some(Arc::new(Mutex::new(state)));
                            }
                            None => {
                                let (stage, hr) = ghostty_vt::gpu_renderer_last_init_error();
                                let stage_name = ghostty_vt::gpu_init_stage_name(stage);
                                log::warn!(
                                    "DX12 GPU renderer unavailable — falling back to GPUI text shaping \
                                     (failed at stage {}={}, HRESULT=0x{:08X})",
                                    stage, stage_name, hr as u32
                                );
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            } else {
                log::info!("DX12 GPU terminal renderer disabled; using GPUI text shaping.");
            }
        }

        let blink_entity = cx.entity().downgrade();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                if blink_entity
                    .update(cx, |pane: &mut TerminalPane, cx| {
                        if matches!(pane.state, TerminalState::Running)
                            && pane.cursor_blink
                            && pane.snapshot.cursor_visible
                        {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            surface,
            focus_handle,
            input_suppressed: settings.input_suppressed.clone(),
            state: TerminalState::Pending,
            snapshot: TerminalSnapshot::new(settings.rows),
            cell_width: measured_w,
            cell_height: measured_h,
            font_family,
            font_size: settings.font_size,
            cursor_blink: settings.cursor_blink,
            copy_on_select: settings.copy_on_select,
            gpu_acceleration: settings.gpu_acceleration,
            fg_color: settings.fg_color,
            bg_color: settings.bg_color,
            background_image_path: settings.background_image_path.clone(),
            background_image_opacity: settings.background_image_opacity,
            global_hotkeys: settings.global_hotkeys.clone(),
            blink_started_at: Instant::now(),
            term_cols: settings.cols,
            term_rows: settings.rows,
            last_width: 0.0,
            last_height: 0.0,
            last_bounds: None,
            selection_anchor: None,
            selection_head: None,
            is_selecting: false,
            ime_composing: false,
            wheel_scroll_line_remainder: 0.0,
            pending_input: Vec::new(),
            pending_process_exit_status: None,
            glyph_cache: Default::default(),
            #[cfg(feature = "ghostty_vt")]
            gpu_state: None,
            #[cfg(not(feature = "ghostty_vt"))]
            row_generations: vec![0; settings.rows as usize],
        }
    }

    fn cursor_blink_visible(&self) -> bool {
        if !self.cursor_blink {
            return true;
        }

        (self.blink_started_at.elapsed().as_millis() / 500).is_multiple_of(2)
    }

    #[cfg(feature = "ghostty_vt")]
    fn recreate_gpu_state(&mut self, cx: &mut Context<Self>) {
        if !self.gpu_acceleration {
            self.gpu_state = None;
            return;
        }

        let width = ((self.term_cols as f32 * self.cell_width) + HORIZONTAL_TEXT_PADDING * 2.0)
            .ceil() as u32;
        let height = (self.term_rows as f32 * self.cell_height).ceil() as u32;
        let width = width.max(64);
        let height = height.max(64);
        let font_size = self.font_size;
        let this = cx.entity().downgrade();
        self.gpu_state = None;
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let gpu_result = cx
                .background_executor()
                .spawn(async move { GpuTerminalState::new(width, height, font_size) })
                .await;

            let _ = this.update(cx, |pane: &mut TerminalPane, cx| {
                if let Some(state) = gpu_result {
                    pane.gpu_state = Some(Arc::new(Mutex::new(state)));
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(not(feature = "ghostty_vt"))]
    fn recreate_gpu_state(&mut self, _cx: &mut Context<Self>) {}

    pub fn take_process_exit_status(&mut self) -> Option<i32> {
        self.pending_process_exit_status.take()
    }

    pub fn clear_history(&mut self) {
        self.surface.clear_history();
        self.snapshot = TerminalSnapshot::new(self.term_rows);
        self.selection_anchor = None;
        self.selection_head = None;
        self.is_selecting = false;
        self.pending_process_exit_status = None;
    }

    pub fn update_settings(&mut self, settings: &TerminalSettings, cx: &mut Context<Self>) {
        let next_font_family = SharedString::from(settings.font_family.clone());
        let font_changed = self.font_family != next_font_family
            || (self.font_size - settings.font_size).abs() > f32::EPSILON;
        let colors_changed =
            self.fg_color != settings.fg_color || self.bg_color != settings.bg_color;
        let blink_changed = self.cursor_blink != settings.cursor_blink;
        let background_image_changed = self.background_image_path != settings.background_image_path
            || (self.background_image_opacity - settings.background_image_opacity).abs()
                > f32::EPSILON;

        self.input_suppressed = settings.input_suppressed.clone();
        if self.input_suppressed.load(Ordering::Relaxed) {
            self.ime_composing = false;
        }
        self.font_family = next_font_family;
        self.font_size = settings.font_size;
        self.cursor_blink = settings.cursor_blink;
        self.copy_on_select = settings.copy_on_select;
        self.gpu_acceleration = settings.gpu_acceleration;
        self.fg_color = settings.fg_color;
        self.bg_color = settings.bg_color;
        self.background_image_path = settings.background_image_path.clone();
        self.background_image_opacity = settings.background_image_opacity;
        self.global_hotkeys = settings.global_hotkeys.clone();
        self.blink_started_at = Instant::now();
        self.surface
            .set_default_colors(self.fg_color, self.bg_color);

        if font_changed {
            let (cell_width, cell_height) =
                measure_cell_dimensions(cx, self.font_family.as_ref(), self.font_size);
            self.cell_width = cell_width;
            self.cell_height = cell_height;
            #[cfg(feature = "ghostty_vt")]
            self.glyph_cache.lock().clear();
            self.recreate_gpu_state(cx);
            if self.last_width > 0.0 && self.last_height > 0.0 {
                let _ = self.handle_resize(self.last_width, self.last_height);
            }
        }

        if font_changed || colors_changed || blink_changed || background_image_changed {
            self.refresh_snapshot(true);
            cx.notify();
        }
    }

    fn render_background_image(&self) -> Option<AnyElement> {
        let image_path = self.background_image_path.as_ref()?;
        if image_path.trim().is_empty() || self.background_image_opacity <= 0.0 {
            return None;
        }

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .overflow_hidden()
                .child(
                    img(PathBuf::from(image_path))
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .opacity(self.background_image_opacity),
                )
                .into_any_element(),
        )
    }

    /// Recalculate terminal grid size from pixel dimensions
    fn handle_resize(&mut self, width_px: f32, height_px: f32) -> bool {
        if (width_px - self.last_width).abs() < 2.0 && (height_px - self.last_height).abs() < 2.0 {
            return false;
        }
        self.last_width = width_px;
        self.last_height = height_px;

        let content_width = (width_px - HORIZONTAL_TEXT_PADDING * 2.0).max(self.cell_width);
        let new_cols = (content_width / self.cell_width).floor().max(1.0) as u16;
        let new_rows = (height_px / self.cell_height).floor().max(1.0) as u16;

        if new_cols != self.term_cols || new_rows != self.term_rows {
            self.term_cols = new_cols;
            self.term_rows = new_rows;
            self.surface.resize(new_cols, new_rows);
            self.snapshot.resize(new_rows);
            // Clear glyph cache on resize — grid geometry changed
            #[cfg(feature = "ghostty_vt")]
            self.glyph_cache.lock().clear();
            #[cfg(not(feature = "ghostty_vt"))]
            self.row_generations.resize(new_rows as usize, 0);
            return true;
        }
        false
    }

    fn refresh_snapshot(&mut self, force_full: bool) -> bool {
        self.snapshot.damaged_rows.clear();
        #[cfg(feature = "ghostty_vt")]
        let (rows, cursor_x, cursor_y, cursor_visible, row_updates) = {
            let mut backend = self.surface.backend.lock();
            let rows = backend.rows;
            let pos = backend.cursor_position();
            let cursor_visible = backend.cursor_visible();
            let scroll_delta = if force_full {
                0
            } else {
                backend.terminal.take_viewport_scroll_delta()
            };
            let dirty_rows = viewport_rows_to_refresh(
                rows,
                force_full,
                scroll_delta,
                match backend.terminal.take_dirty_viewport_rows(rows) {
                    Ok(dirty_rows) => Some(dirty_rows),
                    Err(err) => {
                        log::debug!("Failed to collect dirty terminal rows: {}", err);
                        None
                    }
                },
            );
            let row_updates = dirty_rows
                .into_iter()
                .map(|row| GhosttyRowUpdate {
                    row,
                    text: backend.row_text(row),
                    style_runs: backend.row_style_runs(row),
                })
                .collect::<Vec<_>>();
            (
                rows,
                pos.0.saturating_sub(1),
                pos.1.saturating_sub(1),
                cursor_visible,
                row_updates,
            )
        };

        #[cfg(not(feature = "ghostty_vt"))]
        let (rows, cursor_x, cursor_y, cursor_visible, row_updates) = {
            let backend = self.surface.backend.lock();
            let rows = backend.rows;
            let (cursor_x, cursor_y) = backend.cursor_position();
            let cursor_visible = backend.cursor_visible();
            let dirty_rows: Vec<u16> = if force_full {
                (0..rows).collect()
            } else {
                let mut dirty_rows = Vec::new();
                for row in 0..rows as usize {
                    let next_generation = backend
                        .screen
                        .row_generations
                        .get(row)
                        .copied()
                        .unwrap_or(0);
                    if self.row_generations.get(row).copied().unwrap_or(0) != next_generation {
                        dirty_rows.push(row as u16);
                    }
                }
                dirty_rows
            };
            let row_updates = dirty_rows
                .into_iter()
                .map(|row| {
                    let index = row as usize;
                    FallbackRowUpdate {
                        row,
                        text: backend.row_text(row),
                        generation: backend
                            .screen
                            .row_generations
                            .get(index)
                            .copied()
                            .unwrap_or(0),
                    }
                })
                .collect::<Vec<_>>();
            (rows, cursor_x, cursor_y, cursor_visible, row_updates)
        };

        let cursor_changed = self.snapshot.cursor_x != cursor_x
            || self.snapshot.cursor_y != cursor_y
            || self.snapshot.cursor_visible != cursor_visible;
        let mut changed = cursor_changed;
        let mut content_changed = false;

        if self.snapshot.rows.len() != rows as usize {
            self.snapshot.resize(rows);
            changed = true;
            content_changed = true;
        }

        #[cfg(feature = "ghostty_vt")]
        for row_update in row_updates {
            let index = row_update.row as usize;
            if index >= self.snapshot.rows.len() {
                continue;
            }

            let cached_row = &mut self.snapshot.rows[index];
            let row_changed = apply_ghostty_row_update(
                cached_row,
                row_update,
                self.term_cols,
                self.fg_color,
                self.bg_color,
                force_full,
            );
            changed |= row_changed;
            if row_changed {
                self.snapshot.damaged_rows.push(index as u16);
                content_changed = true;
            }
        }

        #[cfg(not(feature = "ghostty_vt"))]
        for row_update in row_updates {
            let index = row_update.row as usize;
            if index >= self.snapshot.rows.len() {
                continue;
            }

            let cached_row = &mut self.snapshot.rows[index];
            let row_changed = cached_row.text.as_ref() != row_update.text.as_str()
                || self.row_generations.get(index).copied().unwrap_or(0) != row_update.generation;
            cached_row.text = SharedString::from(row_update.text);
            self.row_generations[index] = row_update.generation;
            changed |= row_changed;
            if row_changed {
                self.snapshot.damaged_rows.push(index as u16);
                content_changed = true;
            }
        }

        self.snapshot.cursor_x = cursor_x;
        self.snapshot.cursor_y = cursor_y;
        self.snapshot.cursor_visible = cursor_visible;
        if content_changed {
            self.snapshot.content_revision = self.snapshot.content_revision.wrapping_add(1);
        }
        changed
    }

    /// Create a canvas element that registers the IME input handler during paint
    fn ime_canvas(&self, cx: &mut Context<Self>) -> Canvas<()> {
        let entity = cx.entity().clone();
        let focus = self.focus_handle.clone();
        let suppressed = self.input_suppressed.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let _ = entity.update(cx, |pane, _cx| {
                    pane.last_bounds = Some(bounds);
                });
                // Don't register IME handler when input is suppressed
                // (e.g., group editor overlay is open)
                if !suppressed.load(Ordering::Relaxed) {
                    let handler = ElementInputHandler::new(bounds, entity.clone());
                    window.handle_input(&focus, handler, cx);
                }
            },
        )
        .size_full()
    }

    /// Render the "Connecting..." placeholder (with key buffering support)
    fn render_pending(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ime = self.ime_canvas(cx);
        div()
            .id("terminal-pane")
            .size_full()
            .bg(rgb(self.bg_color))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_drop(cx.listener(Self::on_external_paths_drop))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(SUBTEXT0))
                            .child("Starting shell..."),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(SURFACE0))
                            .child("Initializing ConPTY"),
                    ),
            )
            .child(ime)
    }

    /// Render the error state
    fn render_failed(&self, error: &str) -> impl IntoElement {
        div()
            .id("terminal-pane")
            .size_full()
            .bg(rgb(self.bg_color))
            .track_focus(&self.focus_handle)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(RED))
                            .child("Failed to start shell"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(SUBTEXT0))
                            .child(error.to_string()),
                    ),
            )
    }

    /// Render the running terminal using canvas paint API for pixel-perfect grid.
    fn render_running(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (layout_width, layout_height) =
            terminal_layout_size(window.viewport_size(), self.last_bounds);
        if self.handle_resize(layout_width, layout_height) {
            self.refresh_snapshot(true);
        }

        let selection = self.selection_range();
        let mut render_snapshot = self.snapshot.clone();
        render_snapshot.cursor_visible &= self.cursor_blink_visible();

        let ime = self.ime_canvas(cx);
        let config = GridRendererConfig {
            cell_width: self.cell_width,
            cell_height: self.cell_height,
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            horizontal_text_padding: HORIZONTAL_TEXT_PADDING,
            term_cols: self.term_cols,
            fg_color: self.fg_color,
            bg_color: self.bg_color,
        };

        // Choose rendering path: DX12 native swapchain when possible, otherwise GPUI text shaping.
        #[cfg(feature = "ghostty_vt")]
        let terminal_element: AnyElement = if let Some(ref gpu) = self.gpu_state {
            let cursor = if render_snapshot.cursor_visible {
                render_snapshot
                    .rows
                    .get(render_snapshot.cursor_y as usize)
                    .map(|row| {
                        let (col, width) = resolve_cursor_cell(
                            render_snapshot.cursor_x,
                            &row.cells,
                            self.term_cols,
                        );
                        CursorOverlay {
                            row: render_snapshot.cursor_y,
                            col,
                            width: width as f32,
                        }
                    })
            } else {
                None
            };
            #[cfg(target_os = "windows")]
            if self.background_image_path.is_none()
                && snapshot_can_present_natively(&render_snapshot)
            {
                gpu_terminal_canvas(
                    render_snapshot.clone(),
                    cursor,
                    selection,
                    config,
                    gpu.clone(),
                    self.glyph_cache.clone(),
                )
                .into_any_element()
            } else {
                gpu.lock().hide_native_presenter();
                terminal_canvas(
                    render_snapshot.clone(),
                    selection,
                    config,
                    self.glyph_cache.clone(),
                )
                .into_any_element()
            }
            #[cfg(not(target_os = "windows"))]
            {
                terminal_canvas(
                    render_snapshot.clone(),
                    selection,
                    config,
                    self.glyph_cache.clone(),
                )
                .into_any_element()
            }
        } else {
            terminal_canvas(
                render_snapshot.clone(),
                selection,
                config,
                self.glyph_cache.clone(),
            )
            .into_any_element()
        };
        #[cfg(not(feature = "ghostty_vt"))]
        let terminal_element: AnyElement = terminal_canvas(
            render_snapshot.clone(),
            selection,
            config,
            self.glyph_cache.clone(),
        )
        .into_any_element();

        let mut pane = div()
            .image_cache(retain_all("terminal-background-image-cache"))
            .id("terminal-pane")
            .size_full()
            .bg(rgb(self.bg_color))
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_drop(cx.listener(Self::on_external_paths_drop));

        if let Some(background) = self.render_background_image() {
            pane = pane.child(background);
        }

        pane.child(terminal_element).child(ime)
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.state {
            TerminalState::Pending => self.render_pending(cx).into_any_element(),
            TerminalState::Failed(err) => {
                let err = err.clone();
                self.render_failed(&err).into_any_element()
            }
            TerminalState::Running => self.render_running(window, cx).into_any_element(),
        }
    }
}

impl TerminalPane {
    pub fn send_input(&self, data: &[u8]) -> std::io::Result<usize> {
        self.surface.write_input(data)
    }

    fn selection_range(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let start = self.selection_anchor?;
        let end = self.selection_head?;
        if start == end {
            None
        } else if start <= end {
            Some((start, end))
        } else {
            Some((end, start))
        }
    }

    fn clear_selection(&mut self) -> bool {
        let had_selection =
            self.selection_anchor.is_some() || self.selection_head.is_some() || self.is_selecting;
        self.selection_anchor = None;
        self.selection_head = None;
        self.is_selecting = false;
        had_selection
    }

    fn set_selection_state(
        &mut self,
        selection_anchor: Option<SelectionPoint>,
        selection_head: Option<SelectionPoint>,
        is_selecting: bool,
    ) -> bool {
        let changed = self.selection_anchor != selection_anchor
            || self.selection_head != selection_head
            || self.is_selecting != is_selecting;
        self.selection_anchor = selection_anchor;
        self.selection_head = selection_head;
        self.is_selecting = is_selecting;
        changed
    }

    fn mouse_to_selection_point(&self, position: Point<Pixels>) -> Option<SelectionPoint> {
        let bounds = self.last_bounds?;
        let local = bounds.localize(&position)?;
        let row_count = self.snapshot.rows.len();
        if row_count == 0 {
            return None;
        }

        let x = f32::from(local.x);
        let y = f32::from(local.y);
        let row = (y.max(0.0) / self.cell_height)
            .floor()
            .clamp(0.0, (row_count.saturating_sub(1)) as f32) as u16;
        let col = ((x - HORIZONTAL_TEXT_PADDING).max(0.0) / self.cell_width).floor() as u16;

        Some(SelectionPoint {
            row,
            col: col.min(self.term_cols),
        })
    }

    fn selection_cols_for_row(&self, row: u16) -> Option<(u16, u16)> {
        let (start, end) = self.selection_range()?;
        if row < start.row || row > end.row {
            return None;
        }

        let start_col = if row == start.row { start.col } else { 0 };
        let end_col = if row == end.row {
            end.col
        } else {
            self.term_cols
        };

        if start_col >= end_col {
            None
        } else {
            Some((start_col, end_col.min(self.term_cols)))
        }
    }

    fn selection_overlay(&self, row: u16, cell_w: f32, cell_h: f32) -> Option<Div> {
        let (start_col, end_col) = self.selection_cols_for_row(row)?;
        let width_cols = end_col.saturating_sub(start_col);
        if width_cols == 0 {
            return None;
        }

        Some(
            div()
                .absolute()
                .top_0()
                .left(px(HORIZONTAL_TEXT_PADDING + cell_w * start_col as f32))
                .h(px(cell_h))
                .w(px(cell_w * width_cols as f32))
                .bg(rgba(((SELECTION_BG << 8) | 0x55) as u32))
                .rounded(px(2.0)),
        )
    }

    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        let mut lines = Vec::new();

        for row in start.row..=end.row {
            let row_text = self.snapshot.rows.get(row as usize)?.text.as_ref();
            let text = if start.row == end.row {
                slice_text_by_cols(row_text, start.col as usize, end.col as usize)
            } else if row == start.row {
                slice_text_by_cols(row_text, start.col as usize, usize::MAX)
            } else if row == end.row {
                slice_text_by_cols(row_text, 0, end.col as usize)
            } else {
                row_text.to_string()
            };
            lines.push(text);
        }

        Some(lines.join("\n"))
    }

    fn copy_selection_to_clipboard(&self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.selected_text() else {
            return false;
        };
        if text.is_empty() {
            return false;
        }

        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn write_terminal_bytes(&mut self, data: &[u8]) {
        match self.state {
            TerminalState::Running => {
                let _ = self.surface.write_input(data);
            }
            TerminalState::Pending => self.pending_input.extend_from_slice(data),
            TerminalState::Failed(_) => {}
        }
    }

    fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return false;
        };

        let bytes = normalize_terminal_newlines(&text);
        if bytes.is_empty() {
            return false;
        }

        let selection_changed = self.clear_selection();
        self.write_terminal_bytes(&bytes);
        selection_changed
    }

    fn on_external_paths_drop(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = format_dropped_paths(paths);
        if text.is_empty() {
            return;
        }

        window.focus(&self.focus_handle);
        let selection_changed = self.clear_selection();
        self.write_terminal_bytes(text.as_bytes());
        if selection_changed {
            cx.notify();
        }
    }

    /// Convert a keystroke to bytes for PTY. Returns None if the key should not
    /// be sent (e.g., modifier-only keys).
    fn keystroke_to_bytes(ks: &Keystroke) -> Option<Vec<u8>> {
        // IME VK_PROCESSKEY: hook already called TranslateMessage.
        // Only defer when this event is still part of IME processing.
        let ime_processkey_pending = IME_VK_PROCESSKEY.swap(false, Ordering::AcqRel);
        if should_defer_keystroke_to_ime(ks, ime_processkey_pending) {
            return None;
        }

        // key_char: actual character from ToUnicode (shift/layout aware)
        if let Some(ref kc) = ks.key_char {
            if !kc.is_empty() {
                if ks.modifiers.alt {
                    let mut buf = vec![0x1b];
                    buf.extend_from_slice(kc.as_bytes());
                    return Some(buf);
                }
                return Some(kc.as_bytes().to_vec());
            }
        }

        let key_str = ks.key.as_ref();
        let ctrl = ks.modifiers.control;
        let alt = ks.modifiers.alt;

        match key_str {
            "enter" => Some(b"\r".to_vec()),
            "backspace" => Some(vec![0x7f]),
            "tab" => Some(b"\t".to_vec()),
            "escape" => Some(vec![0x1b]),
            "space" => {
                if alt {
                    Some(vec![0x1b, b' '])
                } else {
                    Some(vec![b' '])
                }
            }
            "up" => Some(b"\x1b[A".to_vec()),
            "down" => Some(b"\x1b[B".to_vec()),
            "right" => Some(b"\x1b[C".to_vec()),
            "left" => Some(b"\x1b[D".to_vec()),
            "home" => Some(b"\x1b[H".to_vec()),
            "end" => Some(b"\x1b[F".to_vec()),
            "pageup" => Some(b"\x1b[5~".to_vec()),
            "pagedown" => Some(b"\x1b[6~".to_vec()),
            "delete" => Some(b"\x1b[3~".to_vec()),
            "insert" => Some(b"\x1b[2~".to_vec()),
            "f1" => Some(b"\x1bOP".to_vec()),
            "f2" => Some(b"\x1bOQ".to_vec()),
            "f3" => Some(b"\x1bOR".to_vec()),
            "f4" => Some(b"\x1bOS".to_vec()),
            "f5" => Some(b"\x1b[15~".to_vec()),
            "f6" => Some(b"\x1b[17~".to_vec()),
            "f7" => Some(b"\x1b[18~".to_vec()),
            "f8" => Some(b"\x1b[19~".to_vec()),
            "f9" => Some(b"\x1b[20~".to_vec()),
            "f10" => Some(b"\x1b[21~".to_vec()),
            "f11" => Some(b"\x1b[23~".to_vec()),
            "f12" => Some(b"\x1b[24~".to_vec()),
            _ => {
                if ctrl && key_str.len() == 1 {
                    let ch = key_str.chars().next().unwrap();
                    if ch.is_ascii_lowercase() {
                        Some(vec![ch as u8 - b'a' + 1])
                    } else {
                        None
                    }
                } else if key_str.len() == 1 {
                    // Fallback: single-char key without key_char (shouldn't normally happen)
                    Some(key_str.as_bytes().to_vec())
                } else {
                    None
                }
            }
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }

        if self
            .global_hotkeys
            .iter()
            .any(|hotkey| crate::app::hotkey_matches(event, hotkey))
        {
            return;
        }

        let key: &str = event.keystroke.key.as_ref();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control && !modifiers.alt && key == "c" && self.copy_selection_to_clipboard(cx)
        {
            cx.stop_propagation();
            return;
        }
        if modifiers.control && !modifiers.alt && key == "v" {
            if self.paste_from_clipboard(cx) {
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }

        let bytes = match Self::keystroke_to_bytes(&event.keystroke) {
            Some(b) => b,
            None => {
                // IME key or unknown — stop propagation to prevent gpui's
                // broken TranslateMessage from interfering with IME.
                cx.stop_propagation();
                return;
            }
        };

        match self.state {
            TerminalState::Running => {
                let selection_changed = self.clear_selection();
                let _ = self.surface.write_input(&bytes);
                if selection_changed {
                    cx.notify();
                }
                cx.stop_propagation();
            }
            TerminalState::Pending => {
                // Buffer input while PTY is connecting
                let selection_changed = self.clear_selection();
                self.pending_input.extend_from_slice(&bytes);
                if selection_changed {
                    cx.notify();
                }
                cx.stop_propagation();
            }
            TerminalState::Failed(_) => {}
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }
        if let Some(point) = self.mouse_to_selection_point(event.position) {
            if self.set_selection_state(Some(point), Some(point), true) {
                cx.notify();
            }
        } else {
            if self.clear_selection() {
                cx.notify();
            }
        }
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some(point) = self.mouse_to_selection_point(event.position) {
            if self.selection_head != Some(point) {
                self.selection_head = Some(point);
                cx.notify();
            }
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_selecting {
            return;
        }
        let prev_anchor = self.selection_anchor;
        let prev_head = self.selection_head;
        let was_selecting = self.is_selecting;
        if let Some(point) = self.mouse_to_selection_point(event.position) {
            self.selection_head = Some(point);
        }
        self.is_selecting = false;
        let should_notify = if self.selection_range().is_none() {
            self.clear_selection()
        } else {
            prev_anchor != self.selection_anchor
                || prev_head != self.selection_head
                || was_selecting != self.is_selecting
        };
        if should_notify {
            if self.copy_on_select && self.selection_range().is_some() {
                let _ = self.copy_selection_to_clipboard(cx);
            }
            cx.notify();
        }
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }
        if !matches!(self.state, TerminalState::Running) {
            return;
        }

        let delta_y = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => {
                let pixels: f32 = delta.y.into();
                pixels
            }
        };
        if delta_y == 0.0 {
            return;
        }

        let line_delta = scroll_lines_from_wheel_delta(
            event.delta,
            self.cell_height,
            &mut self.wheel_scroll_line_remainder,
        );
        if line_delta == 0 {
            cx.stop_propagation();
            return;
        }

        if self.surface.scroll_viewport(line_delta) {
            if self.refresh_snapshot(false) {
                cx.notify();
            }
            cx.stop_propagation();
        }
    }
}

impl Focusable for TerminalPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── IME support via EntityInputHandler ──────────────────────────────
// Receives committed text from Windows IME (Japanese/Chinese/Korean)
// via gpui's WM_IME_COMPOSITION → replace_text_in_range path.

impl EntityInputHandler for TerminalPane {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.ime_composing = false;
    }

    /// Committed text from IME → send to PTY
    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }
        if text.is_empty() {
            self.ime_composing = false;
            return;
        }
        self.ime_composing = false;
        IME_VK_PROCESSKEY.store(false, Ordering::Release);
        match self.state {
            TerminalState::Running => {
                let _ = self.surface.write_input(text.as_bytes());
            }
            TerminalState::Pending => {
                self.pending_input.extend_from_slice(text.as_bytes());
            }
            TerminalState::Failed(_) => {}
        }
    }

    /// Preedit (composing) text from IME — currently not displayed,
    /// but accepting the call prevents gpui from dropping the composition.
    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _new_text: &str,
        _new_selected: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }
        self.ime_composing = !_new_text.is_empty();
        IME_VK_PROCESSKEY.store(false, Ordering::Release);
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if !self.ime_composing {
            return None;
        }
        // Position IME candidate window near cursor
        Some(Bounds::new(
            point(
                element_bounds.origin.x
                    + px(HORIZONTAL_TEXT_PADDING + self.snapshot.cursor_x as f32 * self.cell_width),
                element_bounds.origin.y + px(self.snapshot.cursor_y as f32 * self.cell_height),
            ),
            size(px(self.cell_width), px(self.cell_height)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl EventEmitter<()> for TerminalPane {}

fn slice_text_by_cols(text: &str, start_col: usize, end_col: usize) -> String {
    let start_idx = col_to_char_index(text, start_col);
    let end_idx = if end_col == usize::MAX {
        text.chars().count()
    } else {
        col_to_char_index(text, end_col)
    };

    text.chars()
        .skip(start_idx)
        .take(end_idx.saturating_sub(start_idx))
        .collect()
}

#[cfg(test)]
mod snapshot_tests {
    use super::{
        scroll_lines_from_wheel_delta, should_defer_keystroke_to_ime, terminal_layout_size,
        viewport_rows_to_refresh,
    };
    use gpui::{Bounds, Keystroke, Modifiers, ScrollDelta, point, px, size};

    #[test]
    fn viewport_rows_to_refresh_returns_dirty_rows_without_scroll() {
        assert_eq!(
            viewport_rows_to_refresh(4, false, 0, Some(vec![1, 3])),
            vec![1, 3]
        );
    }

    #[test]
    fn viewport_rows_to_refresh_forces_full_refresh_when_viewport_scrolled() {
        assert_eq!(
            viewport_rows_to_refresh(4, false, 1, Some(vec![3])),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn viewport_rows_to_refresh_falls_back_to_full_refresh_on_dirty_row_failure() {
        assert_eq!(viewport_rows_to_refresh(3, false, 0, None), vec![0, 1, 2]);
    }

    #[test]
    fn should_defer_keystroke_to_ime_only_for_non_text_events() {
        let process_key = Keystroke {
            modifiers: Modifiers::default(),
            key: "ime-process".into(),
            key_char: None,
        };
        let ascii_key = Keystroke {
            modifiers: Modifiers::default(),
            key: "a".into(),
            key_char: Some("a".into()),
        };

        assert!(should_defer_keystroke_to_ime(&process_key, true));
        assert!(!should_defer_keystroke_to_ime(&ascii_key, true));
        assert!(!should_defer_keystroke_to_ime(&ascii_key, false));
    }

    #[test]
    fn scroll_lines_from_wheel_delta_preserves_line_steps() {
        let mut remainder = 0.0;
        assert_eq!(
            scroll_lines_from_wheel_delta(
                ScrollDelta::Lines(point(0.0, 3.0)),
                20.0,
                &mut remainder
            ),
            -3
        );
        assert_eq!(remainder, 0.0);
    }

    #[test]
    fn scroll_lines_from_wheel_delta_accumulates_fractional_pixels() {
        let mut remainder = 0.0;
        assert_eq!(
            scroll_lines_from_wheel_delta(
                ScrollDelta::Pixels(point(px(0.0), px(9.0))),
                20.0,
                &mut remainder
            ),
            0
        );
        assert!(remainder < -0.4 && remainder > -0.5);

        assert_eq!(
            scroll_lines_from_wheel_delta(
                ScrollDelta::Pixels(point(px(0.0), px(11.0))),
                20.0,
                &mut remainder
            ),
            -1
        );
        assert_eq!(remainder, 0.0);
    }

    #[test]
    fn terminal_layout_size_prefers_last_terminal_bounds() {
        let (width, height) = terminal_layout_size(
            size(px(1200.0), px(900.0)),
            Some(Bounds::new(
                point(px(10.0), px(20.0)),
                size(px(840.0), px(640.0)),
            )),
        );

        assert_eq!(width, 840.0);
        assert_eq!(height, 640.0);
    }

    #[test]
    fn terminal_layout_size_falls_back_to_viewport_minus_chrome() {
        let (width, height) = terminal_layout_size(size(px(1200.0), px(900.0)), None);

        assert_eq!(width, 1200.0);
        assert_eq!(height, 840.0);
    }
}

#[cfg(all(test, feature = "ghostty_vt"))]
mod tests {
    use super::*;
    use crate::terminal::grid_renderer::CachedTerminalRow;
    use crate::terminal::{DEFAULT_BG, DEFAULT_FG};

    fn style_run(start_col: u16, end_col: u16, bg_rgb: u32) -> ghostty_vt::StyleRun {
        ghostty_vt::StyleRun {
            start_col,
            end_col,
            fg: ghostty_vt::Rgb {
                r: ((DEFAULT_FG >> 16) & 0xFF) as u8,
                g: ((DEFAULT_FG >> 8) & 0xFF) as u8,
                b: (DEFAULT_FG & 0xFF) as u8,
            },
            bg: ghostty_vt::Rgb {
                r: ((bg_rgb >> 16) & 0xFF) as u8,
                g: ((bg_rgb >> 8) & 0xFF) as u8,
                b: (bg_rgb & 0xFF) as u8,
            },
            flags: 0,
        }
    }

    #[::core::prelude::v1::test]
    fn apply_ghostty_row_update_skips_redraw_for_identical_row() {
        let style_runs = vec![style_run(1, 2, DEFAULT_BG)];
        let cells = grid_cells_from_parts("ab", &style_runs, 4, DEFAULT_FG, DEFAULT_BG);
        let mut row = CachedTerminalRow {
            text: SharedString::from("ab"),
            style_runs: style_runs.clone(),
            cells: cells.clone(),
            glyph_instances: super::super::grid_renderer::glyph_instances_from_cells(&cells, 0),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let changed = apply_ghostty_row_update(
            &mut row,
            GhosttyRowUpdate {
                row: 0,
                text: "ab".into(),
                style_runs,
            },
            4,
            DEFAULT_FG,
            DEFAULT_BG,
            false,
        );

        assert!(!changed);
        assert!(row.damage_spans.is_empty());
        assert!(row.damaged_glyph_instances.is_empty());
    }

    #[::core::prelude::v1::test]
    fn apply_ghostty_row_update_marks_style_only_changes_as_damage() {
        let previous_style_runs = vec![style_run(2, 2, DEFAULT_BG)];
        let cells = grid_cells_from_parts("ab", &previous_style_runs, 4, DEFAULT_FG, DEFAULT_BG);
        let mut row = CachedTerminalRow {
            text: SharedString::from("ab"),
            style_runs: previous_style_runs,
            cells: cells.clone(),
            glyph_instances: super::super::grid_renderer::glyph_instances_from_cells(&cells, 0),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let changed = apply_ghostty_row_update(
            &mut row,
            GhosttyRowUpdate {
                row: 0,
                text: "ab".into(),
                style_runs: vec![style_run(2, 2, 0x224466)],
            },
            4,
            DEFAULT_FG,
            DEFAULT_BG,
            false,
        );

        assert!(changed);
        assert_eq!(
            row.damage_spans,
            vec![super::super::grid_renderer::DamageSpan {
                start_col: 1,
                end_col: 2,
            }]
        );
    }
}
