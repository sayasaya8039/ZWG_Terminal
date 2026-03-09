//! Application state and root view

use gpui::*;
use uuid::Uuid;

use crate::terminal::TerminalPane;

/// Global application state
pub struct AppState {
    pub tabs: Vec<TabInfo>,
    pub active_tab: usize,
}

pub struct TabInfo {
    pub id: Uuid,
    pub title: String,
    pub shell: String,
}

impl AppState {
    pub fn new(_cx: &mut App) -> Self {
        let default_shell = crate::shell::detect_default_shell();
        Self {
            tabs: vec![TabInfo {
                id: Uuid::new_v4(),
                title: "Terminal".to_string(),
                shell: default_shell,
            }],
            active_tab: 0,
        }
    }
}

/// Root view containing tab bar + terminal pane
pub struct RootView {
    state: Entity<AppState>,
    terminal_pane: Entity<TerminalPane>,
}

impl RootView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let shell = state.read(cx).tabs[0].shell.clone();
        let terminal_pane = cx.new(|cx| TerminalPane::new(&shell, cx));
        Self {
            state,
            terminal_pane,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_title = {
            let state = self.state.read(cx);
            state
                .tabs
                .get(state.active_tab)
                .map(|t| t.title.clone())
                .unwrap_or_else(|| "Terminal".to_string())
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e2e))
            .child(self.render_tab_bar(&tab_title))
            .child(
                div()
                    .flex_1()
                    .child(self.terminal_pane.clone()),
            )
    }
}

impl RootView {
    fn render_tab_bar(&self, title: &str) -> impl IntoElement {
        div()
            .h(px(36.0))
            .w_full()
            .flex()
            .items_center()
            .bg(rgb(0x181825))
            .border_b_1()
            .border_color(rgb(0x313244))
            .child(
                div()
                    .px(px(16.0))
                    .py(px(6.0))
                    .mx(px(4.0))
                    .rounded(px(6.0))
                    .bg(rgb(0x1e1e2e))
                    .text_color(rgb(0xcdd6f4))
                    .text_size(px(13.0))
                    .child(title.to_string()),
            )
    }
}
