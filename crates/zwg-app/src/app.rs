//! Application state and root view — multi-tab + split pane support

use std::time::Instant;

use gpui::*;
use uuid::Uuid;

use crate::config::{AppConfig, WindowState};
use crate::shell::{self, ShellType};
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

/// Shell entry for the dropdown menu
#[derive(Clone)]
pub struct ShellEntry {
    pub shell_type: ShellType,
    pub command: String,
    pub display_name: String,
}

/// Global application state
pub struct AppState {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub config: AppConfig,
    pub available_shells: Vec<ShellEntry>,
}

impl AppState {
    pub fn new(cx: &mut App) -> Self {
        let config = AppConfig::load();
        let id = Uuid::new_v4();
        let shell = config.shell.clone();
        let split = cx.new(|cx| SplitContainer::new(&shell, cx));

        // Cache available shells at startup
        let available_shells = shell::detect_available_shells()
            .into_iter()
            .map(|(st, cmd)| {
                let display_name = match st {
                    ShellType::PowerShell => "Windows PowerShell".to_string(),
                    ShellType::Pwsh => "PowerShell 7".to_string(),
                    ShellType::Cmd => "Command Prompt".to_string(),
                    ShellType::Wsl => "WSL".to_string(),
                    ShellType::GitBash => "Git Bash".to_string(),
                };
                ShellEntry {
                    shell_type: st,
                    command: cmd,
                    display_name,
                }
            })
            .collect();

        Self {
            tabs: vec![Tab {
                id,
                title: "Terminal 1".to_string(),
                shell,
                split,
            }],
            active_tab: 0,
            config,
            available_shells,
        }
    }

    pub fn add_tab(&mut self, cx: &mut App) {
        let id = Uuid::new_v4();
        let shell = self.config.shell.clone();
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

    pub fn add_tab_with_shell(&mut self, shell: &str, cx: &mut App) {
        let id = Uuid::new_v4();
        let idx = self.tabs.len() + 1;
        let split = cx.new(|cx| SplitContainer::new(shell, cx));

        self.tabs.push(Tab {
            id,
            title: format!("Terminal {}", idx),
            shell: shell.to_string(),
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
    show_shell_menu: bool,
    /// Cached window bounds — saved to disk periodically and on Drop
    last_bounds: Option<WindowState>,
    /// Last time window state was saved (for debouncing)
    last_save_time: Instant,
    /// Whether bounds have changed since last save
    bounds_dirty: bool,
}

/// Minimum interval between window state saves (seconds)
const WINDOW_STATE_SAVE_INTERVAL_SECS: u64 = 2;

impl RootView {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            show_shell_menu: false,
            last_bounds: None,
            last_save_time: Instant::now(),
            bounds_dirty: false,
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Track window bounds and save periodically (debounced)
        let bounds = window.bounds();
        let new_state = WindowState {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized: window.is_maximized(),
        };

        // Check if bounds changed
        if let Some(ref prev) = self.last_bounds {
            if (prev.x - new_state.x).abs() > 1.0
                || (prev.y - new_state.y).abs() > 1.0
                || (prev.width - new_state.width).abs() > 1.0
                || (prev.height - new_state.height).abs() > 1.0
            {
                self.bounds_dirty = true;
            }
        } else {
            self.bounds_dirty = true;
        }
        self.last_bounds = Some(new_state);

        // Debounced save: write to disk at most every N seconds when dirty
        if self.bounds_dirty
            && self.last_save_time.elapsed().as_secs() >= WINDOW_STATE_SAVE_INTERVAL_SECS
        {
            if let Some(ref state) = self.last_bounds {
                if let Err(e) = state.save() {
                    log::warn!("Failed to save window state: {}", e);
                }
            }
            self.bounds_dirty = false;
            self.last_save_time = Instant::now();
        }

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
        let shell_name = state
            .tabs
            .get(active_tab)
            .map(|t| t.shell.clone())
            .unwrap_or_default();

        // Collect pane count from active split
        let pane_count = active_split
            .as_ref()
            .map(|s| s.read(cx).all_terminals().len())
            .unwrap_or(1);

        let _ = state; // release borrow

        // Build tab elements inline — cx.listener() needs &mut Context<Self>
        let mut tab_elements: Vec<AnyElement> = Vec::new();

        for (idx, title, is_active) in &tab_infos {
            let idx = *idx;
            let is_active = *is_active;

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
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        this.state.update(cx, |s, _cx| {
                            s.active_tab = idx;
                        });
                        cx.notify();
                    }),
                );

            if is_active {
                tab = tab.bg(rgb(BASE)).text_color(rgb(TEXT));
            } else {
                tab = tab
                    .bg(rgb(MANTLE))
                    .text_color(rgb(SUBTEXT0))
                    .hover(|s| s.bg(rgb(SURFACE0)));
            }

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
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                this.state.update(cx, |s, _cx| {
                                    s.close_tab(idx);
                                });
                                cx.notify();
                            }),
                        )
                        .child("×"),
                );
            }

            tab_elements.push(tab.into_any_element());
        }

        // New tab button (+)
        let new_tab_btn = div()
            .id("new-tab-btn")
            .px(px(8.0))
            .py(px(5.0))
            .rounded_l(px(6.0))
            .cursor_pointer()
            .text_size(px(14.0))
            .text_color(rgb(SURFACE1))
            .hover(|s| s.text_color(rgb(GREEN)).bg(rgb(SURFACE0)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_shell_menu = false;
                    this.state.update(cx, |s, cx| {
                        s.add_tab(cx);
                    });
                    cx.notify();
                }),
            )
            .child("+");

        // Shell dropdown button (▽)
        let dropdown_btn = div()
            .id("shell-dropdown-btn")
            .px(px(4.0))
            .py(px(5.0))
            .rounded_r(px(6.0))
            .cursor_pointer()
            .text_size(px(10.0))
            .text_color(rgb(SURFACE1))
            .hover(|s| s.text_color(rgb(GREEN)).bg(rgb(SURFACE0)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_shell_menu = !this.show_shell_menu;
                    cx.notify();
                }),
            )
            .child("▾");

        // Combined + and ▽ button group
        let btn_group = div()
            .ml(px(2.0))
            .flex()
            .items_center()
            .child(new_tab_btn)
            .child(
                div()
                    .h(px(16.0))
                    .w(px(1.0))
                    .bg(rgb(SURFACE0)),
            )
            .child(dropdown_btn);

        // Shell dropdown menu (overlay)
        let show_menu = self.show_shell_menu;
        let shell_entries: Vec<(usize, String, String)> = self
            .state
            .read(cx)
            .available_shells
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e.display_name.clone(), e.command.clone()))
            .collect();

        let tab_bar = div()
            .id("tab-bar")
            .h(px(36.0))
            .w_full()
            .flex()
            .items_center()
            .bg(rgb(MANTLE))
            .border_b_1()
            .border_color(rgb(SURFACE0))
            .children(tab_elements)
            .child(btn_group);

        // Build shell dropdown menu if open
        let shell_menu = if show_menu {
            let mut menu_items: Vec<AnyElement> = Vec::new();
            for (idx, display_name, command) in shell_entries {
                let cmd = command.clone();
                menu_items.push(
                    div()
                        .id(ElementId::Name(format!("shell-item-{}", idx).into()))
                        .px(px(12.0))
                        .py(px(6.0))
                        .w_full()
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(rgb(TEXT))
                        .hover(|s| s.bg(rgb(SURFACE0)))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                this.show_shell_menu = false;
                                let cmd = cmd.clone();
                                this.state.update(cx, |s, cx| {
                                    s.add_tab_with_shell(&cmd, cx);
                                });
                                cx.notify();
                            }),
                        )
                        .child(display_name)
                        .into_any_element(),
                );
            }

            Some(
                div()
                    .id("shell-dropdown-menu")
                    .absolute()
                    .top(px(36.0))
                    .right(px(0.0))
                    .w(px(200.0))
                    .bg(rgb(MANTLE))
                    .border_1()
                    .border_color(rgb(SURFACE0))
                    .rounded(px(6.0))
                    .shadow_lg()
                    .py(px(4.0))
                    .children(menu_items),
            )
        } else {
            None
        };

        // Click-outside overlay to close the menu
        let backdrop = if show_menu {
            Some(
                div()
                    .id("shell-menu-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.show_shell_menu = false;
                            cx.notify();
                        }),
                    ),
            )
        } else {
            None
        };

        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BASE))
            .relative()
            // Action handlers on the root element
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_split_right))
            .on_action(cx.listener(Self::on_split_down))
            .on_action(cx.listener(Self::on_close_pane))
            .on_action(cx.listener(Self::on_focus_next))
            .on_action(cx.listener(Self::on_focus_prev))
            // Tab bar
            .child(tab_bar)
            // Active pane area
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .children(active_split),
            )
            // Status bar
            .child(Self::render_status_bar(&shell_name, pane_count))
            // Shell dropdown overlay (backdrop + menu)
            .children(backdrop)
            .children(shell_menu)
    }
}

impl RootView {
    fn render_status_bar(shell: &str, pane_count: usize) -> impl IntoElement {
        let shell_display = shell
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(shell)
            .replace(".exe", "");

        let version = env!("CARGO_PKG_VERSION");

        let pane_info = if pane_count > 1 {
            format!("{}P", pane_count)
        } else {
            String::new()
        };

        div()
            .h(px(24.0))
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.0))
            .bg(rgb(MANTLE))
            .border_t_1()
            .border_color(rgb(SURFACE0))
            .text_size(px(11.0))
            .text_color(rgb(SUBTEXT0))
            .child({
                // Left: shell name + pane count
                let left = div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(shell_display);
                if pane_info.is_empty() {
                    left
                } else {
                    left.child(
                        div()
                            .text_color(rgb(SURFACE1))
                            .child(pane_info),
                    )
                }
            })
            .child(
                // Right: version
                div()
                    .text_color(rgb(SURFACE1))
                    .child(format!("ZWG v{}", version)),
            )
    }
}

impl Drop for RootView {
    fn drop(&mut self) {
        if let Some(ref state) = self.last_bounds {
            if let Err(e) = state.save() {
                log::warn!("Failed to save window state: {}", e);
            } else {
                log::info!("Window state saved on exit");
            }
        }
    }
}
