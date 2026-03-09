//! Application state and root view — multi-tab + split pane support

use gpui::*;
use uuid::Uuid;

use crate::split::{FocusDir, SplitContainer, SplitDirection};
use crate::{ClosePane, CloseTab, FocusNext, FocusPrev, NewTab, SplitDown, SplitRight};

// Catppuccin Mocha palette
const BASE: u32 = 0x1e1e2e;
const MANTLE: u32 = 0x181825;
const SURFACE0: u32 = 0x313244;
const SURFACE1: u32 = 0x45475a;
const TEXT: u32 = 0xcdd6f4;
const SUBTEXT0: u32 = 0xa6adc8;
const RED: u32 = 0xf38ba8;
const GREEN: u32 = 0xa6e3a1;

/// Per-tab state
pub struct Tab {
    pub id: Uuid,
    pub title: String,
    pub shell: String,
    pub split: Entity<SplitContainer>,
}

/// Global application state
pub struct AppState {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub default_shell: String,
}

impl AppState {
    pub fn new(cx: &mut App) -> Self {
        let default_shell = crate::shell::detect_default_shell();
        let id = Uuid::new_v4();
        let shell = default_shell.clone();
        let split = cx.new(|cx| SplitContainer::new(&shell, cx));

        Self {
            tabs: vec![Tab {
                id,
                title: "Terminal 1".to_string(),
                shell,
                split,
            }],
            active_tab: 0,
            default_shell,
        }
    }

    pub fn add_tab(&mut self, cx: &mut App) {
        let id = Uuid::new_v4();
        let shell = self.default_shell.clone();
        let idx = self.tabs.len() + 1;
        let split = cx.new(|cx| SplitContainer::new(&shell, cx));

        self.tabs.push(Tab {
            id,
            title: format!("Terminal {}", idx),
            shell,
            split,
        });
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn close_tab(&mut self, idx: usize) {
        if self.tabs.len() <= 1 {
            return; // keep at least one tab
        }
        self.tabs.remove(idx);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn active_split(&self) -> Option<&Entity<SplitContainer>> {
        self.tabs.get(self.active_tab).map(|t| &t.split)
    }
}

/// Root view containing tab bar + split container
pub struct RootView {
    state: Entity<AppState>,
}

impl RootView {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self { state }
    }

    fn on_new_tab(&mut self, _action: &NewTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.add_tab(cx);
            cx.notify();
        });
    }

    fn on_close_tab(&mut self, _action: &CloseTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            let idx = state.active_tab;
            state.close_tab(idx);
            cx.notify();
        });
    }

    fn on_split_right(
        &mut self,
        _action: &SplitRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.split(SplitDirection::Horizontal, cx));
        }
    }

    fn on_split_down(
        &mut self,
        _action: &SplitDown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.split(SplitDirection::Vertical, cx));
        }
    }

    fn on_close_pane(
        &mut self,
        _action: &ClosePane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| {
                sc.close_focused(cx);
            });
        }
    }

    fn on_focus_next(
        &mut self,
        _action: &FocusNext,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.focus_direction(FocusDir::Next, cx));
        }
    }

    fn on_focus_prev(
        &mut self,
        _action: &FocusPrev,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.focus_direction(FocusDir::Prev, cx));
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let active_tab = state.active_tab;
        let active_split = state.tabs.get(active_tab).map(|t| t.split.clone());

        // Collect tab info for rendering
        let tab_infos: Vec<(usize, String, bool)> = state
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| (i, t.title.clone(), i == active_tab))
            .collect();
        let tab_count = state.tabs.len();

        let _ = state; // release borrow

        let state_entity = self.state.clone();

        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BASE))
            // Action handlers on the root element
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_split_right))
            .on_action(cx.listener(Self::on_split_down))
            .on_action(cx.listener(Self::on_close_pane))
            .on_action(cx.listener(Self::on_focus_next))
            .on_action(cx.listener(Self::on_focus_prev))
            // Tab bar
            .child(Self::render_tab_bar(&tab_infos, tab_count, state_entity))
            // Active pane area
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .children(active_split),
            )
    }
}

impl RootView {
    fn render_tab_bar(
        tabs: &[(usize, String, bool)],
        tab_count: usize,
        state: Entity<AppState>,
    ) -> impl IntoElement {
        let mut tab_elements: Vec<AnyElement> = Vec::new();

        for (idx, title, is_active) in tabs {
            let idx = *idx;
            let is_active = *is_active;
            let state_for_click = state.clone();
            let state_for_close = state.clone();

            let mut tab = div()
                .id(ElementId::Name(format!("tab-{}", idx).into()))
                .px(px(14.0))
                .py(px(5.0))
                .mx(px(2.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .text_size(px(12.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .on_click(move |_event, _window, cx| {
                    state_for_click.update(cx, |s, cx| {
                        s.active_tab = idx;
                        cx.notify();
                    });
                });

            if is_active {
                tab = tab.bg(rgb(BASE)).text_color(rgb(TEXT));
            } else {
                tab = tab
                    .bg(rgb(MANTLE))
                    .text_color(rgb(SUBTEXT0))
                    .hover(|s| s.bg(rgb(SURFACE0)));
            }

            // Tab title
            tab = tab.child(title.clone());

            // Close button (only show if more than 1 tab)
            if tab_count > 1 {
                tab = tab.child(
                    div()
                        .id(ElementId::Name(format!("tab-close-{}", idx).into()))
                        .text_size(px(10.0))
                        .text_color(rgb(SURFACE1))
                        .hover(|s| s.text_color(rgb(RED)))
                        .cursor_pointer()
                        .rounded(px(3.0))
                        .px(px(3.0))
                        .on_click(move |_event, _window, cx| {
                            state_for_close.update(cx, |s, cx| {
                                s.close_tab(idx);
                                cx.notify();
                            });
                        })
                        .child("×"),
                );
            }

            tab_elements.push(tab.into_any_element());
        }

        // New tab button
        let state_for_new = state.clone();
        let new_tab_btn = div()
            .id("new-tab-btn")
            .px(px(8.0))
            .py(px(5.0))
            .mx(px(2.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(14.0))
            .text_color(rgb(SURFACE1))
            .hover(|s| s.text_color(rgb(GREEN)).bg(rgb(SURFACE0)))
            .on_click(move |_event, _window, cx| {
                state_for_new.update(cx, |s, cx| {
                    s.add_tab(cx);
                    cx.notify();
                });
            })
            .child("+");

        div()
            .h(px(36.0))
            .w_full()
            .flex()
            .items_center()
            .bg(rgb(MANTLE))
            .border_b_1()
            .border_color(rgb(SURFACE0))
            .children(tab_elements)
            .child(new_tab_btn)
    }
}
