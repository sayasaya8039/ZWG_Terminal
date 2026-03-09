//! Terminal pane — GPUI view that renders the terminal and handles input

use gpui::*;

use super::surface::TerminalSurface;
use super::{DEFAULT_BG, DEFAULT_FG};

const FONT_FAMILY: &str = "Cascadia Code";
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT_FACTOR: f32 = 1.3;

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
}

impl TerminalPane {
    pub fn new(shell: &str, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let mut surface = TerminalSurface::new(80, 24);

        if let Err(e) = surface.spawn(shell) {
            log::error!("Failed to spawn shell '{}': {}", shell, e);
        }

        // Poll for PTY output every 16ms (~60fps)
        cx.spawn(async move |this: WeakEntity<TerminalPane>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;

                let should_notify = this
                    .update(cx, |pane: &mut TerminalPane, _cx| {
                        let mut dirty = false;
                        while let Ok(_event) = pane.surface.event_rx.try_recv() {
                            dirty = true;
                        }
                        dirty
                    })
                    .unwrap_or(false);

                if should_notify {
                    let _ = this.update(cx, |_pane: &mut TerminalPane, cx| {
                        cx.notify();
                    });
                }
            }
        })
        .detach();

        Self {
            surface,
            focus_handle,
            cell_width: 8.4,
            cell_height: FONT_SIZE * LINE_HEIGHT_FACTOR,
            term_cols: 80,
            term_rows: 24,
        }
    }

    fn handle_resize(&mut self, width_px: f32, height_px: f32) {
        let new_cols = (width_px / self.cell_width).floor().max(1.0) as u16;
        let new_rows = (height_px / self.cell_height).floor().max(1.0) as u16;

        if new_cols != self.term_cols || new_rows != self.term_rows {
            self.term_cols = new_cols;
            self.term_rows = new_rows;
            self.surface.resize(new_cols, new_rows);
        }
    }
}

impl Render for TerminalPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = self.surface.screen.lock();
        let rows = screen.rows as usize;
        let cursor_x = screen.cursor.x;
        let cursor_y = screen.cursor.y;
        let cursor_visible = screen.cursor.visible;

        let mut line_elements: Vec<AnyElement> = Vec::with_capacity(rows);

        for row in 0..rows {
            let text = screen
                .visible_line(row)
                .map(|l| l.text.clone())
                .unwrap_or_default();

            let is_cursor_row = cursor_visible && row == cursor_y as usize;

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
                    let mut t = text.clone();
                    while t.chars().count() <= cursor_col {
                        t.push(' ');
                    }
                    t
                };

                let row_with_text = row_el.child(
                    div().pl(px(4.0)).child(display_text),
                );

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
                line_elements.push(
                    row_el
                        .child(div().pl(px(4.0)).child(display))
                        .into_any_element(),
                );
            }
        }

        drop(screen);

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

impl TerminalPane {
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
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

impl EventEmitter<()> for TerminalPane {}
