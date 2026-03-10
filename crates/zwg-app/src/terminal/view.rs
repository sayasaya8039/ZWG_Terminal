//! Terminal pane — GPUI view that renders the terminal and handles input

use gpui::*;

use super::surface::{TerminalEvent, TerminalSurface};
use crate::config::RenderConfig;

/// M2: Events emitted by TerminalPane
#[derive(Debug, Clone)]
pub enum TerminalPaneEvent {
    ProcessExited(i32),
    TitleChanged(String),
}

/// Terminal pane: GPUI component that wraps a TerminalSurface
pub struct TerminalPane {
    surface: TerminalSurface,
    focus_handle: FocusHandle,
    /// Cached cell dimensions
    cell_width: f32,
    cell_height: f32,
    /// Current terminal size in cells
    term_cols: u16,
    term_rows: u16,
    /// Last known layout size for resize detection
    last_width: f32,
    last_height: f32,
    /// C3: rendering config from AppConfig
    font_family: String,
    font_size: f32,
    fg_color: u32,
    bg_color: u32,
    /// H3/H6: whether the shell process has exited
    exited: bool,
    /// L1: cursor visibility from backend
    cursor_visible: bool,
}

impl TerminalPane {
    /// C3: accept RenderConfig to drive font/color settings
    pub fn new(shell: &str, render_config: &RenderConfig, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let mut surface = TerminalSurface::new(80, 24);

        if let Err(e) = surface.spawn(shell) {
            log::error!("Failed to spawn shell '{}': {}", shell, e);
        }

        let cell_height = render_config.font_size * render_config.line_height;
        // M1: approximate cell_width from font size (monospace ~0.6 ratio)
        let cell_width = render_config.font_size * 0.6;

        // Poll for PTY output every 16ms (~60fps)
        cx.spawn(async move |this: WeakEntity<TerminalPane>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;

                let result = this
                    .update(cx, |pane: &mut TerminalPane, _cx| {
                        let mut dirty = false;
                        while let Ok(event) = pane.surface.event_rx.try_recv() {
                            dirty = true;
                            // H3/H6: handle process exit
                            if let TerminalEvent::ProcessExited(_code) = event {
                                pane.exited = true;
                            }
                        }
                        (dirty, pane.exited)
                    });

                match result {
                    Ok((dirty, exited)) => {
                        if dirty {
                            let _ = this.update(cx, |_pane: &mut TerminalPane, cx| {
                                cx.notify();
                            });
                        }
                        // H6: stop polling after process exits
                        if exited {
                            break;
                        }
                    }
                    Err(_) => break, // Entity dropped
                }
            }
        })
        .detach();

        Self {
            surface,
            focus_handle,
            cell_width,
            cell_height,
            term_cols: 80,
            term_rows: 24,
            last_width: 0.0,
            last_height: 0.0,
            font_family: render_config.font_family.clone(),
            font_size: render_config.font_size,
            fg_color: render_config.fg_color,
            bg_color: render_config.bg_color,
            exited: false,
            cursor_visible: true,
        }
    }

    /// Recalculate terminal grid size from pixel dimensions
    fn handle_resize(&mut self, width_px: f32, height_px: f32) {
        // Avoid resizing for tiny jitter
        if (width_px - self.last_width).abs() < 2.0
            && (height_px - self.last_height).abs() < 2.0
        {
            return;
        }
        self.last_width = width_px;
        self.last_height = height_px;

        let new_cols = (width_px / self.cell_width).floor().max(1.0) as u16;
        let new_rows = (height_px / self.cell_height).floor().max(1.0) as u16;

        if new_cols != self.term_cols || new_rows != self.term_rows {
            self.term_cols = new_cols;
            self.term_rows = new_rows;
            self.surface.resize(new_cols, new_rows);
        }
    }

    /// Get current terminal dimensions for status bar
    pub fn term_size(&self) -> (u16, u16) {
        (self.term_cols, self.term_rows)
    }

    /// Whether the shell process has exited
    pub fn is_exited(&self) -> bool {
        self.exited
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // C4: approximate resize from viewport size.
        // TODO: GPUI lacks on_resize for elements — use proper pane bounds when available.
        let vp = window.viewport_size();
        let vp_w: f32 = vp.width.into();
        let vp_h: f32 = vp.height.into();
        let avail_h = (vp_h - 60.0).max(100.0); // subtract tab bar + status bar
        self.handle_resize(vp_w, avail_h);

        // Snapshot backend state under brief lock — release before building UI
        let rows: u16;
        let cursor_x: u16;
        let cursor_y: u16;
        let cursor_visible: bool;
        let mut row_texts: Vec<String>;
        #[cfg(feature = "ghostty_vt")]
        let row_styles: Vec<Vec<ghostty_vt::StyleRun>>;
        {
            let backend = self.surface.backend.lock();
            rows = backend.rows;
            let pos = backend.cursor_position();
            cursor_x = pos.0;
            cursor_y = pos.1;
            row_texts = (0..rows).map(|r| backend.row_text(r)).collect();
            #[cfg(feature = "ghostty_vt")]
            {
                row_styles = (0..rows).map(|r| backend.row_style_runs(r)).collect();
            }
            // L1: snapshot cursor visibility
            #[cfg(not(feature = "ghostty_vt"))]
            {
                cursor_visible = backend.screen.cursor.visible;
            }
            #[cfg(feature = "ghostty_vt")]
            {
                cursor_visible = true; // ghostty handles cursor visibility internally
            }
        }
        // Mutex released — PTY reader thread unblocked

        self.cursor_visible = cursor_visible;

        let fg = self.fg_color;
        let bg = self.bg_color;
        let font_size = self.font_size;
        let font_family = self.font_family.clone();

        let mut line_elements: Vec<AnyElement> = Vec::with_capacity(rows as usize);

        for row in 0..rows {
            let ri = row as usize;
            let text = std::mem::take(&mut row_texts[ri]);
            let is_cursor_row = row == cursor_y;

            #[cfg(feature = "ghostty_vt")]
            let style_runs = &row_styles[ri];

            let row_el = div()
                .h(px(self.cell_height))
                .w_full()
                .flex()
                .items_center()
                .text_size(px(font_size))
                .font_family(font_family.clone())
                .text_color(rgb(fg));

            // L1: only render cursor overlay when cursor is visible and not exited
            if is_cursor_row && cursor_visible && !self.exited {
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

                #[cfg(feature = "ghostty_vt")]
                let text_child =
                    render_styled_text(&display_text, style_runs, self.cell_width, fg, bg);
                #[cfg(not(feature = "ghostty_vt"))]
                let text_child = div().pl(px(4.0)).child(display_text);

                let row_with_text = row_el.child(text_child);

                let cursor_offset = cursor_col as f32 * self.cell_width + 4.0;
                line_elements.push(
                    div()
                        .h(px(self.cell_height))
                        .w_full()
                        .relative()
                        .child(row_with_text)
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left(px(cursor_offset))
                                .w(px(self.cell_width))
                                .h(px(self.cell_height))
                                .bg(rgba(0xcdd6f480))
                                .rounded(px(1.0)),
                        )
                        .into_any_element(),
                );
            } else {
                let display = if text.is_empty() {
                    " ".to_string()
                } else {
                    text
                };

                #[cfg(feature = "ghostty_vt")]
                let text_child =
                    render_styled_text(&display, style_runs, self.cell_width, fg, bg);
                #[cfg(not(feature = "ghostty_vt"))]
                let text_child = div().pl(px(4.0)).child(display);

                line_elements.push(row_el.child(text_child).into_any_element());
            }
        }

        // H3: show "[Process exited]" when shell has terminated
        if self.exited {
            line_elements.push(
                div()
                    .h(px(self.cell_height))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(font_size))
                    .text_color(rgb(0x585b70))
                    .child("[Process exited]")
                    .into_any_element(),
            );
        }

        div()
            .id("terminal-pane")
            .size_full()
            .bg(rgb(bg))
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .children(line_elements)
    }
}

/// Render a row of text with Ghostty style runs applied
#[cfg(feature = "ghostty_vt")]
fn render_styled_text(
    text: &str,
    style_runs: &[ghostty_vt::StyleRun],
    _cell_width: f32,
    default_fg: u32,
    default_bg: u32,
) -> Div {
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

        // Emit unstyled gap before this run
        if covered_to < start {
            let gap: String = chars[covered_to..start].iter().collect();
            children.push(
                div()
                    .child(gap)
                    .text_color(rgb(default_fg))
                    .into_any_element(),
            );
        }

        let segment: String = chars[start..end].iter().collect();
        let fg = rgb(((run.fg.r as u32) << 16) | ((run.fg.g as u32) << 8) | (run.fg.b as u32));
        let bg_val = ((run.bg.r as u32) << 16) | ((run.bg.g as u32) << 8) | (run.bg.b as u32);

        let mut span = div().child(segment).text_color(fg);

        // Only set background if it differs from the terminal default
        if bg_val != default_bg {
            span = span.bg(rgb(bg_val));
        }

        // Apply text styles from flags
        if run.flags & 0x01 != 0 {
            span = span.font_weight(FontWeight::BOLD);
        }

        children.push(span.into_any_element());
        covered_to = end;
    }

    // Emit trailing unstyled text
    if covered_to < chars.len() {
        let tail: String = chars[covered_to..].iter().collect();
        children.push(
            div()
                .child(tail)
                .text_color(rgb(default_fg))
                .into_any_element(),
        );
    }

    container.children(children)
}

impl TerminalPane {
    fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Don't accept input after process exit
        if self.exited {
            return;
        }

        let ks = &event.keystroke;

        // Try key_char first (IME / composed input)
        if let Some(ref kc) = ks.key_char {
            if !kc.is_empty() {
                let _ = self.surface.write_input(kc.as_bytes());
                cx.notify();
                return;
            }
        }

        // Handle special keys
        let key_str = ks.key.as_ref();
        let ctrl = ks.modifiers.control;
        let alt = ks.modifiers.alt;

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
            // M10: F-keys
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
                    // Safe unwrap replacement
                    if let Some(ch) = key_str.chars().next() {
                        if ch.is_ascii_lowercase() {
                            Some(vec![ch as u8 - b'a' + 1])
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else if alt && key_str.len() == 1 {
                    // M10: Alt+key sends ESC + key
                    let mut seq = vec![0x1b];
                    seq.extend_from_slice(key_str.as_bytes());
                    Some(seq)
                } else if key_str.len() == 1 {
                    Some(key_str.as_bytes().to_vec())
                } else {
                    None
                }
            }
        };

        if let Some(bytes) = data {
            let _ = self.surface.write_input(&bytes);
            cx.notify();
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

// M2: typed event emitter
impl EventEmitter<TerminalPaneEvent> for TerminalPane {}
