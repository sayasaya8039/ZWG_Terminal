//! Terminal pane — GPUI view that renders the terminal and handles input

use std::ops::Range;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::*;
use gpui::ElementInputHandler;

use super::pty::{ConPtyConfig, spawn_pty};
use super::surface::TerminalSurface;
use super::{DEFAULT_BG, DEFAULT_FG, TerminalSettings};

const FONT_FAMILY: &str = "Consolas";
const FONT_SIZE: f32 = 13.0;
const HORIZONTAL_TEXT_PADDING: f32 = 4.0;
/// Fallback values — replaced at runtime by measured font metrics
const CELL_WIDTH_FALLBACK: f32 = 8.4;
const CELL_HEIGHT_FALLBACK: f32 = 19.5;
pub const CELL_WIDTH_ESTIMATE: f32 = CELL_WIDTH_FALLBACK;
pub const CELL_HEIGHT_ESTIMATE: f32 = CELL_HEIGHT_FALLBACK;
pub const WINDOW_CHROME_HEIGHT: f32 = 60.0;

/// Measure the actual monospace cell dimensions from the font at FONT_SIZE.
/// Cell width = advance width of 'M'.
/// Cell height = ascent + descent (no extra leading — required for
/// box-drawing characters │─┌┐└┘ to connect between adjacent rows).
fn measure_cell_dimensions(cx: &App) -> (f32, f32) {
    let text_system = cx.text_system();
    let font_desc = font(FONT_FAMILY);
    let font_id = text_system.resolve_font(&font_desc);
    let font_size = px(FONT_SIZE);

    let cell_width = text_system
        .advance(font_id, font_size, 'M')
        .map(|size| {
            let w: f32 = size.width.into();
            if w > 1.0 { w } else { CELL_WIDTH_FALLBACK }
        })
        .unwrap_or(CELL_WIDTH_FALLBACK);

    let ascent: f32 = text_system.ascent(font_id, font_size).into();
    let descent: f32 = text_system.descent(font_id, font_size).into();
    // descent may be negative (OpenType convention) — use abs
    let cell_height = ascent + descent.abs();
    let cell_height = if cell_height > FONT_SIZE { cell_height } else { CELL_HEIGHT_FALLBACK };

    // Snap to integer pixels — prevents sub-pixel gaps between rows and
    // accumulated horizontal drift. Essential for box-drawing characters.
    (cell_width.ceil(), cell_height.ceil())
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
        CallNextHookEx, TranslateMessage, MSG, WM_KEYDOWN, PM_REMOVE,
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
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_GETMESSAGE};
    use windows::Win32::System::Threading::GetCurrentThreadId;

    static INIT: Once = Once::new();
    INIT.call_once(|| {
        unsafe {
            let thread_id = GetCurrentThreadId();
            match SetWindowsHookExW(WH_GETMESSAGE, Some(ime_getmessage_hook_proc), None, thread_id) {
                Ok(_) => log::info!("IME GetMessage hook installed"),
                Err(e) => log::error!("Failed to install IME hook: {}", e),
            }
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn install_ime_hook() {}

/// Check if a character is fullwidth (CJK, etc.) — occupies 2 terminal columns
fn is_fullwidth(ch: char) -> bool {
    matches!(ch,
        '\u{1100}'..='\u{115F}'    // Hangul Jamo
        | '\u{2E80}'..='\u{303E}'  // CJK Radicals, Kangxi, Ideographic
        | '\u{3040}'..='\u{33BF}'  // Hiragana, Katakana, Bopomofo, CJK Compat
        | '\u{3400}'..='\u{4DBF}'  // CJK Unified Ideographs Extension A
        | '\u{4E00}'..='\u{9FFF}'  // CJK Unified Ideographs
        | '\u{A000}'..='\u{A4CF}'  // Yi
        | '\u{AC00}'..='\u{D7AF}'  // Hangul Syllables
        | '\u{F900}'..='\u{FAFF}'  // CJK Compatibility Ideographs
        | '\u{FE30}'..='\u{FE6F}'  // CJK Compatibility Forms
        | '\u{FF01}'..='\u{FF60}'  // Fullwidth Forms
        | '\u{FFE0}'..='\u{FFE6}'  // Fullwidth Signs
        | '\u{20000}'..='\u{2A6DF}' // CJK Extension B
        | '\u{2A700}'..='\u{2CEAF}' // CJK Extensions C-F
        | '\u{2CEB0}'..='\u{2EBEF}' // CJK Extension F
        | '\u{30000}'..='\u{3134F}' // CJK Extension G
    )
}

/// Convert terminal column position to character index in a string
fn col_to_char_index(text: &str, target_col: usize) -> usize {
    let mut col = 0;
    for (i, ch) in text.chars().enumerate() {
        if col >= target_col {
            return i;
        }
        col += if is_fullwidth(ch) { 2 } else { 1 };
    }
    text.chars().count()
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SelectionPoint {
    row: u16,
    col: u16,
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

#[derive(Clone, Default)]
struct CachedTerminalRow {
    /// SharedString: O(1) clone (Arc-backed) — avoids per-frame String allocation
    text: SharedString,
    #[cfg(feature = "ghostty_vt")]
    style_runs: Vec<ghostty_vt::StyleRun>,
}

struct TerminalSnapshot {
    rows: Vec<CachedTerminalRow>,
    cursor_x: u16,
    cursor_y: u16,
}

impl TerminalSnapshot {
    fn new(rows: u16) -> Self {
        Self {
            rows: vec![CachedTerminalRow::default(); rows as usize],
            cursor_x: 0,
            cursor_y: 0,
        }
    }

    fn resize(&mut self, rows: u16) {
        self.rows
            .resize(rows as usize, CachedTerminalRow::default());
    }
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
    /// Keystrokes buffered while PTY is still connecting (Pending state)
    pending_input: Vec<u8>,
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

        // Wait for PTY output, then coalesce updates to one present every ~16.7ms.
        cx.spawn(
            async move |this: WeakEntity<TerminalPane>, cx: &mut AsyncApp| {
                let frame_budget = std::time::Duration::from_micros(16_667);
                let mut last_presented: Option<std::time::Instant> = None;

                loop {
                    if event_rx.recv_async().await.is_err() {
                        break;
                    }
                    while event_rx.try_recv().is_ok() {}

                    if let Some(last_presented_at) = last_presented {
                        let elapsed = last_presented_at.elapsed();
                        if elapsed < frame_budget {
                            cx.background_executor().timer(frame_budget - elapsed).await;
                        }
                    }

                    let should_notify = match this.update(cx, |pane: &mut TerminalPane, _cx| {
                        pane.refresh_snapshot(false)
                    }) {
                        Ok(should_notify) => should_notify,
                        Err(_) => break,
                    };

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

        let (measured_w, measured_h) = measure_cell_dimensions(cx);
        log::info!(
            "Terminal cell: width={:.2}px height={:.2}px (fallback w={:.1} h={:.1})",
            measured_w, measured_h, CELL_WIDTH_FALLBACK, CELL_HEIGHT_FALLBACK,
        );

        Self {
            surface,
            focus_handle,
            input_suppressed: settings.input_suppressed.clone(),
            state: TerminalState::Pending,
            snapshot: TerminalSnapshot::new(settings.rows),
            cell_width: measured_w,
            cell_height: measured_h,
            term_cols: settings.cols,
            term_rows: settings.rows,
            last_width: 0.0,
            last_height: 0.0,
            last_bounds: None,
            selection_anchor: None,
            selection_head: None,
            is_selecting: false,
            pending_input: Vec::new(),
            #[cfg(not(feature = "ghostty_vt"))]
            row_generations: vec![0; settings.rows as usize],
        }
    }

    /// Recalculate terminal grid size from pixel dimensions
    fn handle_resize(&mut self, width_px: f32, height_px: f32) -> bool {
        if (width_px - self.last_width).abs() < 2.0 && (height_px - self.last_height).abs() < 2.0 {
            return false;
        }
        self.last_width = width_px;
        self.last_height = height_px;

        let new_cols = (width_px / self.cell_width).floor().max(1.0) as u16;
        let new_rows = (height_px / self.cell_height).floor().max(1.0) as u16;

        if new_cols != self.term_cols || new_rows != self.term_rows {
            self.term_cols = new_cols;
            self.term_rows = new_rows;
            self.surface.resize(new_cols, new_rows);
            self.snapshot.resize(new_rows);
            #[cfg(not(feature = "ghostty_vt"))]
            self.row_generations.resize(new_rows as usize, 0);
            return true;
        }
        false
    }

    fn refresh_snapshot(&mut self, force_full: bool) -> bool {
        let mut backend = self.surface.backend.lock();
        let rows = backend.rows;

        #[cfg(feature = "ghostty_vt")]
        let (cursor_x, cursor_y) = {
            let pos = backend.cursor_position();
            (pos.0.saturating_sub(1), pos.1.saturating_sub(1))
        };

        #[cfg(not(feature = "ghostty_vt"))]
        let (cursor_x, cursor_y) = backend.cursor_position();

        let mut changed = self.snapshot.cursor_x != cursor_x || self.snapshot.cursor_y != cursor_y;

        if self.snapshot.rows.len() != rows as usize {
            self.snapshot.resize(rows);
            changed = true;
        }

        #[cfg(feature = "ghostty_vt")]
        let dirty_rows: Vec<u16> = if force_full {
            (0..rows).collect()
        } else {
            match backend.terminal.take_dirty_viewport_rows(rows) {
                Ok(dirty_rows) => dirty_rows,
                Err(err) => {
                    log::debug!("Failed to collect dirty terminal rows: {}", err);
                    (0..rows).collect()
                }
            }
        };

        #[cfg(not(feature = "ghostty_vt"))]
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

        for row in dirty_rows {
            let index = row as usize;
            if index >= self.snapshot.rows.len() {
                continue;
            }

            let cached_row = &mut self.snapshot.rows[index];
            cached_row.text = SharedString::from(backend.row_text(row));
            #[cfg(feature = "ghostty_vt")]
            {
                cached_row.style_runs = backend.row_style_runs(row);
            }
            #[cfg(not(feature = "ghostty_vt"))]
            {
                self.row_generations[index] = backend
                    .screen
                    .row_generations
                    .get(index)
                    .copied()
                    .unwrap_or(0);
            }
            changed = true;
        }

        #[cfg(not(feature = "ghostty_vt"))]
        if force_full {
            for row in 0..self.snapshot.rows.len() {
                self.row_generations[row] = backend
                    .screen
                    .row_generations
                    .get(row)
                    .copied()
                    .unwrap_or(0);
            }
        }

        self.snapshot.cursor_x = cursor_x;
        self.snapshot.cursor_y = cursor_y;
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
            .bg(rgb(DEFAULT_BG))
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
            .bg(rgb(DEFAULT_BG))
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
        // Detect resize from viewport
        let vp = window.viewport_size();
        let vp_w: f32 = vp.width.into();
        let vp_h: f32 = vp.height.into();
        let avail_h = (vp_h - WINDOW_CHROME_HEIGHT).max(100.0);
        if self.handle_resize(vp_w, avail_h) {
            self.refresh_snapshot(true);
        }

        let num_rows = self.snapshot.rows.len();
        let cursor_x = self.snapshot.cursor_x;
        let cursor_y = self.snapshot.cursor_y;
        let cell_h = self.cell_height;
        let cell_w = self.cell_width;
        let term_cols = self.term_cols;

        // Clone snapshot data for canvas closure (SharedString clone is O(1))
        let rows_snapshot: Vec<CachedTerminalRow> = self.snapshot.rows.clone();
        let selection = self.selection_range();

        let ime = self.ime_canvas(cx);

        // Canvas-based terminal rendering — ShapedLine::paint at exact grid positions
        let terminal_canvas = canvas(
            |_, _, _| (),
            move |bounds: Bounds<Pixels>, _, window: &mut Window, cx: &mut App| {
                let text_system = window.text_system().clone();
                let font_desc = font(FONT_FAMILY);
                let font_size = px(FONT_SIZE);
                let line_height_px = px(cell_h);
                let default_fg = Hsla::from(rgb(DEFAULT_FG));

                // 1) Paint selection background (behind text)
                if let Some((sel_start, sel_end)) = selection {
                    let max_row = sel_end.row.min(num_rows.saturating_sub(1) as u16);
                    for row in sel_start.row..=max_row {
                        let sc = if row == sel_start.row { sel_start.col } else { 0 };
                        let ec = if row == sel_end.row { sel_end.col } else { term_cols };
                        if sc >= ec { continue; }
                        window.paint_quad(fill(
                            Bounds::new(
                                point(
                                    bounds.origin.x + px(HORIZONTAL_TEXT_PADDING + sc as f32 * cell_w),
                                    bounds.origin.y + px(row as f32 * cell_h),
                                ),
                                size(px((ec - sc) as f32 * cell_w), line_height_px),
                            ),
                            rgba(0x2F6FED55),
                        ));
                    }
                }

                // 2) Paint text rows
                for (row_idx, row_data) in rows_snapshot.iter().enumerate() {
                    let text = &row_data.text;
                    if text.is_empty() { continue; }

                    let origin = point(
                        bounds.origin.x + px(HORIZONTAL_TEXT_PADDING),
                        bounds.origin.y + px(row_idx as f32 * cell_h),
                    );

                    #[cfg(feature = "ghostty_vt")]
                    let runs = build_canvas_text_runs(
                        text, &row_data.style_runs, &font_desc, default_fg,
                    );
                    #[cfg(not(feature = "ghostty_vt"))]
                    let runs = vec![TextRun {
                        len: text.len(),
                        font: font_desc.clone(),
                        color: default_fg,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }];

                    // force_width = cell_w: snap every glyph to exact grid columns
                    // (prevents box-drawing chars from drifting)
                    let shaped = text_system.shape_line(
                        text.clone(), font_size, &runs, Some(px(cell_w)),
                    );
                    let _ = shaped.paint_background(origin, line_height_px, window, cx);
                    let _ = shaped.paint(origin, line_height_px, window, cx);
                }

                // 3) Paint cursor
                if (cursor_y as usize) < num_rows {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                bounds.origin.x + px(HORIZONTAL_TEXT_PADDING + cursor_x as f32 * cell_w),
                                bounds.origin.y + px(cursor_y as f32 * cell_h),
                            ),
                            size(px(cell_w), line_height_px),
                        ),
                        rgba(0xF5F5F780),
                    ));
                }
            },
        )
        .size_full();

        div()
            .id("terminal-pane")
            .size_full()
            .bg(rgb(DEFAULT_BG))
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_drop(cx.listener(Self::on_external_paths_drop))
            .child(terminal_canvas)
            .child(ime)
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

/// Build TextRun[] from Ghostty style runs for ShapedLine-based canvas rendering.
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

    let chars: Vec<char> = text.chars().collect();
    let char_byte_offsets: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();

    let mut runs: Vec<TextRun> = Vec::new();
    let mut covered_to_byte: usize = 0;

    for run in style_runs {
        let start_char = run.start_col.saturating_sub(1) as usize;
        let end_char = (run.end_col as usize).min(chars.len());
        if start_char >= chars.len() || start_char >= end_char { continue; }

        let byte_start = char_byte_offsets[start_char];
        let byte_end = char_byte_offsets[end_char];

        // Skip if overlapping or already past
        if byte_start < covered_to_byte { continue; }

        // Gap before this run — fill with default style
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
        let bg_val = ((run.bg.r as u32) << 16) | ((run.bg.g as u32) << 8) | (run.bg.b as u32);
        let fg_color = Hsla::from(rgb(fg_val));

        let mut run_font = font_desc.clone();
        if run.flags & 0x01 != 0 { run_font.weight = FontWeight::BOLD; }
        if run.flags & 0x02 != 0 { run_font.style = FontStyle::Italic; }

        runs.push(TextRun {
            len: byte_end - byte_start,
            font: run_font,
            color: fg_color,
            background_color: if bg_val != DEFAULT_BG { Some(Hsla::from(rgb(bg_val))) } else { None },
            underline: if run.flags & 0x04 != 0 {
                Some(UnderlineStyle { thickness: px(1.0), color: Some(fg_color), wavy: false })
            } else { None },
            strikethrough: if run.flags & 0x10 != 0 {
                Some(StrikethroughStyle { thickness: px(1.0), color: Some(fg_color) })
            } else { None },
        });
        covered_to_byte = byte_end;
    }

    // Tail text with default style
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

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_head = None;
        self.is_selecting = false;
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
        let Some(text) = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
        else {
            return false;
        };

        let bytes = normalize_terminal_newlines(&text);
        if bytes.is_empty() {
            return false;
        }

        self.clear_selection();
        self.write_terminal_bytes(&bytes);
        true
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
        self.clear_selection();
        self.write_terminal_bytes(text.as_bytes());
        cx.notify();
    }

    /// Convert a keystroke to bytes for PTY. Returns None if the key should not
    /// be sent (e.g., modifier-only keys).
    fn keystroke_to_bytes(ks: &Keystroke) -> Option<Vec<u8>> {
        // IME VK_PROCESSKEY: hook already called TranslateMessage.
        // Text will arrive via EntityInputHandler::replace_text_in_range.
        if IME_VK_PROCESSKEY.compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
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
                if alt { Some(vec![0x1b, b' ']) } else { Some(vec![b' ']) }
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

        let key: &str = event.keystroke.key.as_ref();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control && !modifiers.alt && key == "c" && self.copy_selection_to_clipboard(cx) {
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
                self.clear_selection();
                let _ = self.surface.write_input(&bytes);
                cx.stop_propagation();
            }
            TerminalState::Pending => {
                // Buffer input while PTY is connecting
                self.clear_selection();
                self.pending_input.extend_from_slice(&bytes);
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
            self.selection_anchor = Some(point);
            self.selection_head = Some(point);
            self.is_selecting = true;
            cx.notify();
        } else {
            self.clear_selection();
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
            self.selection_head = Some(point);
            cx.notify();
        }
    }

    fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_selecting {
            return;
        }
        if let Some(point) = self.mouse_to_selection_point(event.position) {
            self.selection_head = Some(point);
        }
        self.is_selecting = false;
        if self.selection_range().is_none() {
            self.clear_selection();
        }
        cx.notify();
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
        &mut self, _range: Range<usize>, _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window, _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self, _ignore: bool, _window: &mut Window, _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(
        &self, _window: &mut Window, _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Committed text from IME → send to PTY
    fn replace_text_in_range(
        &mut self, _range: Option<Range<usize>>, text: &str,
        _window: &mut Window, cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            return;
        }
        match self.state {
            TerminalState::Running => {
                let _ = self.surface.write_input(text.as_bytes());
            }
            TerminalState::Pending => {
                self.pending_input.extend_from_slice(text.as_bytes());
            }
            TerminalState::Failed(_) => {}
        }
        cx.notify();
    }

    /// Preedit (composing) text from IME — currently not displayed,
    /// but accepting the call prevents gpui from dropping the composition.
    fn replace_and_mark_text_in_range(
        &mut self, _range: Option<Range<usize>>, _new_text: &str,
        _new_selected: Option<Range<usize>>, _window: &mut Window, cx: &mut Context<Self>,
    ) {
        cx.notify();
    }

    fn bounds_for_range(
        &mut self, _range: Range<usize>, element_bounds: Bounds<Pixels>,
        _window: &mut Window, _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Position IME candidate window near cursor
        Some(Bounds::new(
            element_bounds.origin,
            size(px(self.cell_width), px(self.cell_height)),
        ))
    }

    fn character_index_for_point(
        &mut self, _point: Point<Pixels>, _window: &mut Window, _cx: &mut Context<Self>,
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
