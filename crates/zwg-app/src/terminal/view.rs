//! Terminal pane — GPUI view that renders the terminal and handles input

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::*;

use super::pty::{ConPtyConfig, spawn_pty};
use super::surface::TerminalSurface;
use super::{DEFAULT_BG, DEFAULT_FG, TerminalSettings};

const FONT_FAMILY: &str = "Consolas";
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT_FACTOR: f32 = 1.5;
pub const CELL_WIDTH_ESTIMATE: f32 = 8.4;
pub const CELL_HEIGHT_ESTIMATE: f32 = FONT_SIZE * LINE_HEIGHT_FACTOR;
pub const WINDOW_CHROME_HEIGHT: f32 = 60.0;

// Figma-aligned chrome colors for status text
const SUBTEXT0: u32 = 0x8E8E93;
const SURFACE0: u32 = 0x48484A;
const RED: u32 = 0xFF5F57;

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
    text: String,
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
    #[cfg(not(feature = "ghostty_vt"))]
    row_generations: Vec<u64>,
}

impl TerminalPane {
    pub fn new(shell: &str, settings: TerminalSettings, cx: &mut Context<Self>) -> Self {
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

        Self {
            surface,
            focus_handle,
            input_suppressed: settings.input_suppressed.clone(),
            state: TerminalState::Pending,
            snapshot: TerminalSnapshot::new(settings.rows),
            cell_width: CELL_WIDTH_ESTIMATE,
            cell_height: CELL_HEIGHT_ESTIMATE,
            term_cols: settings.cols,
            term_rows: settings.rows,
            last_width: 0.0,
            last_height: 0.0,
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
            cached_row.text = backend.row_text(row);
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

    /// Render the "Connecting..." placeholder
    fn render_pending(&self) -> impl IntoElement {
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

    /// Render the running terminal
    fn render_running(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Detect resize from viewport
        let vp = window.viewport_size();
        let vp_w: f32 = vp.width.into();
        let vp_h: f32 = vp.height.into();
        let avail_h = (vp_h - WINDOW_CHROME_HEIGHT).max(100.0);
        if self.handle_resize(vp_w, avail_h) {
            self.refresh_snapshot(true);
        }

        let rows = self.snapshot.rows.len() as u16;
        let cursor_x = self.snapshot.cursor_x;
        let cursor_y = self.snapshot.cursor_y;

        let mut line_elements: Vec<AnyElement> = Vec::with_capacity(rows as usize);

        for row in 0..rows {
            let ri = row as usize;
            let text = self.snapshot.rows[ri].text.clone();
            let is_cursor_row = row == cursor_y;

            #[cfg(feature = "ghostty_vt")]
            let style_runs = &self.snapshot.rows[ri].style_runs;

            let row_el = div()
                .h(px(self.cell_height))
                .w_full()
                .flex()
                .items_center()
                .text_size(px(FONT_SIZE))
                .font_family(FONT_FAMILY)
                .text_color(rgb(DEFAULT_FG));

            if is_cursor_row {
                let cursor_col = cursor_x as usize;
                let display_text = if text.is_empty() {
                    " ".repeat(cursor_col + 1)
                } else {
                    let mut t = text;
                    while t.chars().count() <= cursor_col {
                        t.push(' ');
                    }
                    t
                };

                let chars: Vec<char> = display_text.chars().collect();
                let before_cursor: String = chars[..cursor_col.min(chars.len())].iter().collect();

                #[cfg(feature = "ghostty_vt")]
                let text_child = render_styled_text(&display_text, style_runs, self.cell_width);
                #[cfg(not(feature = "ghostty_vt"))]
                let text_child = div().pl(px(4.0)).child(display_text);

                let row_with_text = row_el.child(text_child);

                let cursor_overlay = div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h(px(self.cell_height))
                    .w_full()
                    .flex()
                    .items_center()
                    .text_size(px(FONT_SIZE))
                    .font_family(FONT_FAMILY)
                    .overflow_hidden()
                    .child(
                        div()
                            .pl(px(4.0))
                            .text_color(rgba(0x00000000))
                            .child(before_cursor),
                    )
                    .child(
                        div()
                            .w(px(self.cell_width))
                            .h(px(self.cell_height))
                            .bg(rgba(0xF5F5F780))
                            .rounded(px(1.0)),
                    );

                line_elements.push(
                    div()
                        .h(px(self.cell_height))
                        .w_full()
                        .relative()
                        .child(row_with_text)
                        .child(cursor_overlay)
                        .into_any_element(),
                );
            } else {
                let display = if text.is_empty() {
                    " ".to_string()
                } else {
                    text
                };

                #[cfg(feature = "ghostty_vt")]
                let text_child = render_styled_text(&display, style_runs, self.cell_width);
                #[cfg(not(feature = "ghostty_vt"))]
                let text_child = div().pl(px(4.0)).child(display);

                line_elements.push(row_el.child(text_child).into_any_element());
            }
        }

        div()
            .id("terminal-pane")
            .size_full()
            .bg(rgb(DEFAULT_BG))
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .children(line_elements)
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.state {
            TerminalState::Pending => self.render_pending().into_any_element(),
            TerminalState::Failed(err) => {
                let err = err.clone();
                self.render_failed(&err).into_any_element()
            }
            TerminalState::Running => self.render_running(window, cx).into_any_element(),
        }
    }
}

/// Render a row of text with Ghostty style runs applied
#[cfg(feature = "ghostty_vt")]
fn render_styled_text(text: &str, style_runs: &[ghostty_vt::StyleRun], _cell_width: f32) -> Div {
    let container = div().pl(px(4.0)).flex().flex_row();

    if style_runs.is_empty() || text.is_empty() {
        return container.child(text.to_string());
    }

    let chars: Vec<char> = text.chars().collect();
    let mut children: Vec<AnyElement> = Vec::new();
    let mut covered_to: usize = 0;

    for run in style_runs {
        let start = (run.start_col.saturating_sub(1)) as usize;
        let end = run.end_col as usize;
        if start >= chars.len() {
            break;
        }
        let end = end.min(chars.len());

        if covered_to < start {
            let gap: String = chars[covered_to..start].iter().collect();
            children.push(
                div()
                    .child(gap)
                    .text_color(rgb(DEFAULT_FG))
                    .into_any_element(),
            );
        }

        let segment: String = chars[start..end].iter().collect();
        let fg = rgb(((run.fg.r as u32) << 16) | ((run.fg.g as u32) << 8) | (run.fg.b as u32));
        let bg_val = ((run.bg.r as u32) << 16) | ((run.bg.g as u32) << 8) | (run.bg.b as u32);

        let mut span = div().child(segment).text_color(fg);

        if bg_val != DEFAULT_BG {
            span = span.bg(rgb(bg_val));
        }

        if run.flags & 0x01 != 0 {
            span = span.font_weight(FontWeight::BOLD);
        }

        children.push(span.into_any_element());
        covered_to = end;
    }

    if covered_to < chars.len() {
        let tail: String = chars[covered_to..].iter().collect();
        children.push(
            div()
                .child(tail)
                .text_color(rgb(DEFAULT_FG))
                .into_any_element(),
        );
    }

    container.children(children)
}

impl TerminalPane {
    pub fn send_input(&self, data: &[u8]) -> std::io::Result<usize> {
        self.surface.write_input(data)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }

        // Ignore input while PTY is not connected
        if !matches!(self.state, TerminalState::Running) {
            return;
        }

        let ks = &event.keystroke;

        // Try key_char first (IME / composed input)
        if let Some(ref kc) = ks.key_char {
            if !kc.is_empty() {
                let _ = self.surface.write_input(kc.as_bytes());
                return;
            }
        }

        // Handle special keys
        let key_str = ks.key.as_ref();
        let ctrl = ks.modifiers.control;

        let data: Option<Vec<u8>> = match key_str {
            "enter" => Some(b"\r".to_vec()),
            "backspace" => Some(vec![0x7f]),
            "tab" => Some(b"\t".to_vec()),
            "escape" => Some(vec![0x1b]),
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
            _ => {
                if ctrl && key_str.len() == 1 {
                    let ch = key_str.chars().next().unwrap();
                    if ch.is_ascii_lowercase() {
                        Some(vec![ch as u8 - b'a' + 1])
                    } else {
                        None
                    }
                } else if key_str.len() == 1 {
                    Some(key_str.as_bytes().to_vec())
                } else {
                    None
                }
            }
        };

        if let Some(bytes) = data {
            let _ = self.surface.write_input(&bytes);
        }
    }

    fn on_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
    }
}

impl Focusable for TerminalPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for TerminalPane {}
