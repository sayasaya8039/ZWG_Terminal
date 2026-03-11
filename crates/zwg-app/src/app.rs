//! Application state and root view — Figma-aligned macOS terminal chrome

use std::time::Instant;

use gpui::*;

use crate::config::{AppConfig, WindowState, set_launch_on_login};
use crate::shell::{self, ShellType};
use crate::split::{FocusDir, SplitContainer, SplitDirection};
use crate::terminal::TerminalSettings;
use crate::terminal::view::{CELL_HEIGHT_ESTIMATE, CELL_WIDTH_ESTIMATE, WINDOW_CHROME_HEIGHT};
use crate::{ClosePane, CloseTab, FocusNext, FocusPrev, NewTab, SplitDown, SplitRight};

const WINDOW_BG: u32 = 0x1C1C1E;
const TITLEBAR_BG: u32 = 0x2C2C2E;
const PANEL_BG: u32 = 0x323234;
const PANEL_SIDEBAR_BG: u32 = 0x242426;
const SURFACE1: u32 = 0x48484A;
const SURFACE2: u32 = 0x636366;
const TEXT: u32 = 0xF5F5F7;
const TEXT_SOFT: u32 = 0xE5E5EA;
const SUBTEXT0: u32 = 0xC7C7CC;
const SUBTEXT1: u32 = 0x8E8E93;
const MUTED: u32 = 0x636366;
const RED: u32 = 0xFF5F57;
const GREEN: u32 = 0x28C840;
const YELLOW: u32 = 0xFEBC2E;
const ACCENT: u32 = 0x0A84FF;
const ACCENT_ALT: u32 = 0x34C759;
const BACKDROP: u32 = 0x00000088;
const UI_FONT: &str = "Inter";

/// Minimum interval between window state saves.
const WINDOW_STATE_SAVE_INTERVAL_SECS: u64 = 2;

/// Theme preview cards in the settings panel.
const THEME_PREVIEWS: [(&str, u32); 6] = [
    ("ダーク", WINDOW_BG),
    ("ライト", 0xF5F5F7),
    ("Solarized", 0x002B36),
    ("Monokai", 0x272822),
    ("Dracula", 0x282A36),
    ("Nord", 0x2E3440),
];

/// Per-tab state.
pub struct Tab {
    pub title: String,
    pub shell_type: ShellType,
    pub split: Entity<SplitContainer>,
}

/// Shell entry for the shell selector.
#[derive(Clone)]
pub struct ShellEntry {
    pub shell_type: ShellType,
    pub command: String,
    pub display_name: String,
}

/// Global application state.
pub struct AppState {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub available_shells: Vec<ShellEntry>,
    pub config: AppConfig,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsCategory {
    General,
    Appearance,
    Profiles,
    Keyboard,
    Notifications,
    Privacy,
    Advanced,
}

impl SettingsCategory {
    fn all() -> &'static [(SettingsCategory, &'static str, &'static str)] {
        &[
            (Self::General, "一般", "◻"),
            (Self::Appearance, "外観", "◌"),
            (Self::Profiles, "プロファイル", ">_"),
            (Self::Keyboard, "キーボード", "⌨"),
            (Self::Notifications, "通知", "♪"),
            (Self::Privacy, "プライバシー", "⌂"),
            (Self::Advanced, "詳細", "⚙"),
        ]
    }

    fn title(self) -> &'static str {
        match self {
            Self::General => "一般",
            Self::Appearance => "外観",
            Self::Profiles => "プロファイル",
            Self::Keyboard => "キーボード",
            Self::Notifications => "通知",
            Self::Privacy => "プライバシー",
            Self::Advanced => "詳細",
        }
    }
}

/// Root view containing tab bar + split container + overlays.
pub struct RootView {
    state: Entity<AppState>,
    show_shell_menu: bool,
    show_settings: bool,
    show_close_confirm: bool,
    settings_category: SettingsCategory,
    last_bounds: Option<WindowState>,
    last_save_time: Instant,
    bounds_dirty: bool,
}

impl AppState {
    pub fn new(cx: &mut App) -> Self {
        let available_shells: Vec<ShellEntry> = shell::detect_available_shells()
            .into_iter()
            .map(|(shell_type, command)| ShellEntry {
                display_name: shell_display_name(&shell_type).to_string(),
                shell_type,
                command,
            })
            .collect();

        let mut config = AppConfig::load();
        let default_shell = resolve_default_shell_command(&config.shell, &available_shells);
        if config.shell != default_shell {
            config.shell = default_shell.clone();
            if let Err(err) = config.save() {
                log::warn!("Failed to normalize default shell in config: {}", err);
            }
        }

        let (shell_type, title) = shell_meta_for_command(&default_shell, &available_shells);
        let terminal_settings = terminal_settings_from_config(&config);
        let split = cx.new(|cx| SplitContainer::new(&default_shell, terminal_settings, cx));

        Self {
            tabs: vec![Tab {
                title,
                shell_type,
                split,
            }],
            active_tab: 0,
            available_shells,
            config,
        }
    }

    pub fn add_tab(&mut self, cx: &mut App) {
        let shell = resolve_default_shell_command(&self.config.shell, &self.available_shells);
        self.add_tab_with_shell(&shell, cx);
    }

    pub fn add_tab_with_shell(&mut self, shell: &str, cx: &mut App) {
        let (shell_type, title) = shell_meta_for_command(shell, &self.available_shells);
        let terminal_settings = terminal_settings_from_config(&self.config);
        let split = cx.new(|cx| SplitContainer::new(shell, terminal_settings, cx));

        self.tabs.push(Tab {
            title,
            shell_type,
            split,
        });
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn close_tab(&mut self, idx: usize) {
        if self.tabs.len() <= 1 {
            return;
        }
        self.tabs.remove(idx);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn active_split(&self) -> Option<&Entity<SplitContainer>> {
        self.tabs.get(self.active_tab).map(|t| &t.split)
    }

    pub fn apply_config(&mut self, config: AppConfig, cx: &mut App) {
        self.config = config;
        let terminal_settings = terminal_settings_from_config(&self.config);
        for tab in &self.tabs {
            tab.split.update(cx, |split, _cx| {
                split.update_terminal_settings(terminal_settings);
            });
        }
    }
}

impl RootView {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            show_shell_menu: false,
            show_settings: false,
            show_close_confirm: false,
            settings_category: SettingsCategory::General,
            last_bounds: None,
            last_save_time: Instant::now(),
            bounds_dirty: false,
        }
    }

    fn on_new_tab(&mut self, _action: &NewTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_shell_menu = false;
        self.show_settings = false;
        self.state.update(cx, |state, cx| {
            state.add_tab(cx);
            cx.notify();
        });
    }

    fn on_close_tab(&mut self, _action: &CloseTab, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.close_tab(state.active_tab);
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

    fn on_split_down(&mut self, _action: &SplitDown, _window: &mut Window, cx: &mut Context<Self>) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.split(SplitDirection::Vertical, cx));
        }
    }

    fn on_close_pane(&mut self, _action: &ClosePane, _window: &mut Window, cx: &mut Context<Self>) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| {
                sc.close_focused(cx);
            });
        }
    }

    fn on_focus_next(&mut self, _action: &FocusNext, _window: &mut Window, cx: &mut Context<Self>) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.focus_direction(FocusDir::Next, cx));
        }
    }

    fn on_focus_prev(&mut self, _action: &FocusPrev, _window: &mut Window, cx: &mut Context<Self>) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.focus_direction(FocusDir::Prev, cx));
        }
    }

    fn on_quit_requested(
        &mut self,
        _action: &crate::Quit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_window_close(window, cx);
    }

    fn request_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let should_confirm = self.state.read(cx).config.confirm_on_close;
        if should_confirm {
            self.show_shell_menu = false;
            self.show_settings = false;
            self.show_close_confirm = true;
            cx.notify();
        } else {
            window.remove_window();
        }
    }

    fn persist_config_update<F>(&mut self, cx: &mut Context<Self>, mutate: F)
    where
        F: FnOnce(&mut AppConfig),
    {
        let mut config = self.state.read(cx).config.clone();
        mutate(&mut config);
        let config = config.sanitized();

        self.state.update(cx, |state, cx| {
            state.apply_config(config.clone(), cx);
            if let Err(err) = state.config.save() {
                log::warn!("Failed to save config: {}", err);
            }
            cx.notify();
        });
    }

    fn cycle_default_profile(&mut self, cx: &mut Context<Self>) {
        let (available_shells, current_shell) = {
            let state = self.state.read(cx);
            (state.available_shells.clone(), state.config.shell.clone())
        };

        if available_shells.is_empty() {
            return;
        }

        let current_index = available_shells
            .iter()
            .position(|entry| entry.command == current_shell)
            .unwrap_or(0);
        let next_index = (current_index + 1) % available_shells.len();
        let next_shell = available_shells[next_index].command.clone();
        self.persist_config_update(cx, move |config| {
            config.shell = next_shell;
        });
    }

    fn toggle_launch_on_login(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.launch_on_login;
        match set_launch_on_login(next_value) {
            Ok(()) => self.persist_config_update(cx, move |config| {
                config.launch_on_login = next_value;
            }),
            Err(err) => log::warn!("Failed to update launch-on-login setting: {}", err),
        }
    }

    fn toggle_tab_bar_visibility(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.tab_bar_visible;
        self.persist_config_update(cx, move |config| {
            config.tab_bar_visible = next_value;
        });
    }

    fn toggle_confirm_on_close(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.confirm_on_close;
        self.persist_config_update(cx, move |config| {
            config.confirm_on_close = next_value;
        });
    }

    fn adjust_window_grid(
        &mut self,
        cols_delta: i32,
        rows_delta: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_config = self.state.read(cx).config.clone();
        let next_cols =
            ((current_config.default_window_cols as i32) + cols_delta).clamp(60, 240) as u16;
        let next_rows =
            ((current_config.default_window_rows as i32) + rows_delta).clamp(18, 120) as u16;

        self.persist_config_update(cx, move |config| {
            config.default_window_cols = next_cols;
            config.default_window_rows = next_rows;
        });

        if !window.is_maximized() {
            window.resize(window_size_from_grid(next_cols, next_rows));
        }
    }

    fn adjust_scrollback_lines(&mut self, delta: i32, cx: &mut Context<Self>) {
        let current_value = self.state.read(cx).config.scrollback_lines as i32;
        let next_value = (current_value + delta).clamp(100, 100_000) as usize;
        self.persist_config_update(cx, move |config| {
            config.scrollback_lines = next_value;
        });
    }

    fn render_window_traffic_lights(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id("traffic-close")
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(rgb(RED))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.request_window_close(window, cx);
                        }),
                    ),
            )
            .child(
                div()
                    .id("traffic-minimize")
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(rgb(YELLOW)),
            )
            .child(
                div()
                    .id("traffic-maximize")
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(rgb(GREEN)),
            )
    }

    fn render_modal_traffic_lights(&mut self, modal: &'static str, cx: &mut Context<Self>) -> Div {
        let close_modal = move |this: &mut RootView,
                                _: &MouseDownEvent,
                                _window: &mut Window,
                                cx: &mut Context<RootView>| {
            match modal {
                "shell" => this.show_shell_menu = false,
                "settings" => this.show_settings = false,
                _ => {}
            }
            cx.notify();
        };

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id(ElementId::Name(format!("{modal}-close").into()))
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(rgb(RED))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(close_modal)),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("{modal}-min").into()))
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(rgb(YELLOW)),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("{modal}-max").into()))
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(rgb(GREEN)),
            )
    }

    fn render_shell_selector(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        shells: &[ShellEntry],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_shell_menu {
            return None;
        }

        let panel_w = 520.0;
        let panel_h = 420.0;
        let top = ((viewport_h - panel_h) * 0.5).max(24.0);
        let left = ((viewport_w - panel_w) * 0.5).max(24.0);

        let mut shell_items: Vec<AnyElement> = Vec::new();
        for (idx, shell_entry) in shells.iter().cloned().enumerate() {
            let command = shell_entry.command.clone();
            let display_name = shell_entry.display_name.clone();
            let description = shell_description(&shell_entry.shell_type);
            let icon = shell_icon(&shell_entry.shell_type);

            shell_items.push(
                div()
                    .id(ElementId::Name(format!("shell-item-{idx}").into()))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(10.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(0xffffff12)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            this.show_shell_menu = false;
                            this.state.update(cx, |state, cx| {
                                state.add_tab_with_shell(&command, cx);
                                cx.notify();
                            });
                        }),
                    )
                    .child(
                        div()
                            .w(px(36.0))
                            .h(px(36.0))
                            .rounded(px(10.0))
                            .bg(rgba(0xffffff18))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(17.0))
                            .child(icon),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_size(px(13.0))
                                    .text_color(rgb(TEXT))
                                    .child(display_name),
                            )
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_size(px(11.0))
                                    .text_color(rgb(SUBTEXT1))
                                    .child(description),
                            ),
                    )
                    .child(div().text_size(px(13.0)).text_color(rgb(MUTED)).child(">"))
                    .into_any_element(),
            );
        }

        Some(
            div()
                .id("shell-selector")
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_shell_menu = false;
                    cx.notify();
                }))
                .absolute()
                .top(px(top))
                .left(px(left))
                .w(px(panel_w))
                .h(px(panel_h))
                .flex()
                .flex_col()
                .rounded(px(12.0))
                .overflow_hidden()
                .shadow_lg()
                .bg(rgb(PANEL_BG))
                .border_1()
                .border_color(rgba(0xffffff16))
                .child(
                    div()
                        .h(px(44.0))
                        .w_full()
                        .px(px(16.0))
                        .border_b_1()
                        .border_color(rgba(0xffffff10))
                        .flex()
                        .items_center()
                        .child(self.render_modal_traffic_lights("shell", cx))
                        .child(div().flex_1())
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(13.0))
                                .text_color(rgb(TEXT))
                                .child("New Terminal"),
                        )
                        .child(div().flex_1()),
                )
                .child(
                    div()
                        .flex_1()
                        .p(px(16.0))
                        .flex()
                        .flex_col()
                        .gap(px(14.0))
                        .child(
                            div()
                                .h(px(36.0))
                                .rounded(px(10.0))
                                .bg(rgba(0xffffff0f))
                                .border_1()
                                .border_color(rgba(0xffffff10))
                                .px(px(12.0))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(SUBTEXT1))
                                        .child("/"),
                                )
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(13.0))
                                        .text_color(rgb(MUTED))
                                        .child("Search shells..."),
                                ),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(11.0))
                                .text_color(rgb(SUBTEXT1))
                                .child("AVAILABLE SHELLS"),
                        )
                        .child(
                            div()
                                .id("shell-list-scroll")
                                .w_full()
                                .h(px(176.0))
                                .max_h(px(176.0))
                                .overflow_scroll()
                                .scrollbar_width(px(6.0))
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .children(shell_items),
                                ),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(11.0))
                                .text_color(rgb(SUBTEXT1))
                                .pt(px(4.0))
                                .child("ACTIONS"),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .px(px(12.0))
                                .py(px(10.0))
                                .rounded(px(10.0))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgba(0xffffff12)))
                                .child(
                                    div()
                                        .w(px(36.0))
                                        .h(px(36.0))
                                        .rounded(px(10.0))
                                        .bg(rgba(0x0A84FF33))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(16.0))
                                        .text_color(rgb(ACCENT))
                                        .child(">_"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .font_family(UI_FONT)
                                        .text_size(px(13.0))
                                        .text_color(rgb(ACCENT))
                                        .child("Install New Shell..."),
                                )
                                .child(div().text_size(px(13.0)).text_color(rgb(MUTED)).child(">")),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_settings_panel(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_settings {
            return None;
        }

        let panel_w = 740.0;
        let panel_h = 540.0;
        let top = ((viewport_h - panel_h) * 0.5).max(24.0);
        let left = ((viewport_w - panel_w) * 0.5).max(24.0);

        let mut category_items: Vec<AnyElement> = Vec::new();
        for (category, label, icon) in SettingsCategory::all() {
            let active = *category == self.settings_category;
            let category_value = *category;
            let mut button = div()
                .id(ElementId::Name(format!("settings-{label}").into()))
                .w_full()
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(10.0))
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        this.settings_category = category_value;
                        cx.notify();
                    }),
                );

            if active {
                button = button.bg(rgb(ACCENT)).text_color(rgb(0xffffff));
            } else {
                button = button
                    .text_color(rgb(TEXT_SOFT))
                    .hover(|style| style.bg(rgba(0xffffff10)));
            }

            category_items.push(
                button
                    .child(div().w(px(16.0)).child(*icon))
                    .child(*label)
                    .into_any_element(),
            );
        }

        Some(
            div()
                .id("settings-panel")
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_settings = false;
                    cx.notify();
                }))
                .absolute()
                .top(px(top))
                .left(px(left))
                .w(px(panel_w))
                .h(px(panel_h))
                .rounded(px(12.0))
                .overflow_hidden()
                .shadow_lg()
                .bg(rgb(PANEL_BG))
                .border_1()
                .border_color(rgba(0xffffff16))
                .flex()
                .child(
                    div()
                        .w(px(200.0))
                        .h_full()
                        .bg(rgb(PANEL_SIDEBAR_BG))
                        .border_r_1()
                        .border_color(rgba(0xffffff10))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(46.0))
                                .px(px(16.0))
                                .border_b_1()
                                .border_color(rgba(0xffffff10))
                                .flex()
                                .items_center()
                                .child(self.render_modal_traffic_lights("settings", cx)),
                        )
                        .child(
                            div()
                                .p(px(8.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .children(category_items),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(46.0))
                                .px(px(24.0))
                                .border_b_1()
                                .border_color(rgba(0xffffff10))
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(16.0))
                                        .text_color(rgb(TEXT))
                                        .child(self.settings_category.title()),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .p(px(24.0))
                                .children(Some(self.render_settings_content(window, cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_close_confirm_dialog(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_close_confirm {
            return None;
        }

        let panel_w = 360.0;
        let panel_h = 180.0;
        let top = ((viewport_h - panel_h) * 0.5).max(24.0);
        let left = ((viewport_w - panel_w) * 0.5).max(24.0);

        Some(
            div()
                .id("close-confirm-dialog")
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_close_confirm = false;
                    cx.notify();
                }))
                .absolute()
                .top(px(top))
                .left(px(left))
                .w(px(panel_w))
                .h(px(panel_h))
                .rounded(px(12.0))
                .border_1()
                .border_color(rgba(0xffffff16))
                .bg(rgb(PANEL_BG))
                .shadow_lg()
                .p(px(20.0))
                .flex()
                .flex_col()
                .gap(px(18.0))
                .child(
                    div()
                        .font_family(UI_FONT)
                        .text_size(px(18.0))
                        .text_color(rgb(TEXT))
                        .child("ZWG Terminal を終了しますか？"),
                )
                .child(
                    div()
                        .font_family(UI_FONT)
                        .text_size(px(13.0))
                        .text_color(rgb(SUBTEXT1))
                        .child("実行中のセッションはそのまま終了します。"),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .w_full()
                        .flex()
                        .justify_end()
                        .gap(px(10.0))
                        .child(interactive_action_button(
                            "キャンセル",
                            false,
                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                this.show_close_confirm = false;
                                cx.notify();
                            }),
                        ))
                        .child(interactive_action_button(
                            "終了",
                            true,
                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                this.show_close_confirm = false;
                                cx.notify();
                                window.remove_window();
                            }),
                        )),
                )
                .into_any_element(),
        )
    }

    fn render_settings_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.settings_category {
            SettingsCategory::General => {
                self.render_general_settings(window, cx).into_any_element()
            }
            SettingsCategory::Appearance => self.render_appearance_settings().into_any_element(),
            SettingsCategory::Profiles => self.render_profiles_settings().into_any_element(),
            SettingsCategory::Keyboard => self.render_keyboard_settings().into_any_element(),
            SettingsCategory::Notifications => {
                self.render_notifications_settings().into_any_element()
            }
            SettingsCategory::Privacy => self.render_privacy_settings().into_any_element(),
            SettingsCategory::Advanced => self.render_advanced_settings().into_any_element(),
        }
    }

    fn render_general_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let (config, available_shells) = {
            let state = self.state.read(cx);
            (state.config.clone(), state.available_shells.clone())
        };
        let default_profile_name = available_shells
            .iter()
            .find(|entry| entry.command == config.shell)
            .map(|entry| entry.display_name.clone())
            .unwrap_or_else(|| shell_meta_for_command(&config.shell, &available_shells).1);

        div()
            .flex()
            .flex_col()
            .gap(px(24.0))
            .child(settings_section_heading("起動"))
            .child(settings_row(
                "既定のプロファイル",
                interactive_select_box(
                    default_profile_name,
                    150.0,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.cycle_default_profile(cx);
                    }),
                ),
            ))
            .child(settings_row(
                "ログイン時に起動",
                interactive_toggle(
                    config.launch_on_login,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_launch_on_login(cx);
                    }),
                ),
            ))
            .child(settings_row(
                "タブバーを表示",
                interactive_toggle(
                    config.tab_bar_visible,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_tab_bar_visibility(cx);
                    }),
                ),
            ))
            .child(settings_row(
                "終了前に確認",
                interactive_toggle(
                    config.confirm_on_close,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_confirm_on_close(cx);
                    }),
                ),
            ))
            .child(section_divider())
            .child(settings_section_heading("ウィンドウ"))
            .child(settings_row(
                "新規ウィンドウのサイズ",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(number_stepper(
                        config.default_window_cols.to_string(),
                        64.0,
                        false,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.adjust_window_grid(-1, 0, window, cx);
                        }),
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.adjust_window_grid(1, 0, window, cx);
                        }),
                    ))
                    .child(
                        div()
                            .font_family(UI_FONT)
                            .text_size(px(13.0))
                            .text_color(rgb(SUBTEXT1))
                            .child("x"),
                    )
                    .child(number_stepper(
                        config.default_window_rows.to_string(),
                        64.0,
                        false,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.adjust_window_grid(0, -1, window, cx);
                        }),
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.adjust_window_grid(0, 1, window, cx);
                        }),
                    )),
            ))
            .child(settings_row(
                "スクロールバック行数",
                number_stepper(
                    config.scrollback_lines.to_string(),
                    96.0,
                    false,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.adjust_scrollback_lines(-1_000, cx);
                    }),
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.adjust_scrollback_lines(1_000, cx);
                    }),
                ),
            ))
    }

    fn render_appearance_settings(&self) -> Div {
        let mut theme_rows_top: Vec<AnyElement> = Vec::new();
        let mut theme_rows_bottom: Vec<AnyElement> = Vec::new();

        for (idx, (name, preview)) in THEME_PREVIEWS.iter().enumerate() {
            let card = theme_card(name, *preview, idx == 0).into_any_element();
            if idx < 3 {
                theme_rows_top.push(card);
            } else {
                theme_rows_bottom.push(card);
            }
        }

        div()
            .flex()
            .flex_col()
            .gap(px(24.0))
            .child(settings_section_heading("テーマ"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(div().flex().gap(px(12.0)).children(theme_rows_top))
                    .child(div().flex().gap(px(12.0)).children(theme_rows_bottom)),
            )
            .child(section_divider())
            .child(settings_section_heading("フォント"))
            .child(settings_row(
                "フォントファミリー",
                select_box("JetBrains Mono", 180.0),
            ))
            .child(settings_row(
                "フォントサイズ",
                slider_with_value(0.45, "13px"),
            ))
            .child(section_divider())
            .child(settings_section_heading("カーソル"))
            .child(settings_row(
                "カーソル形状",
                segmented_control(&[("バー", true), ("ブロック", false), ("下線", false)]),
            ))
            .child(settings_row("カーソル点滅", toggle(true)))
            .child(section_divider())
            .child(settings_section_heading("透明効果"))
            .child(settings_row(
                "ウィンドウ不透明度",
                slider_with_value(0.95, "95%"),
            ))
            .child(settings_row("背景ぼかし", slider_with_value(0.33, "10px")))
    }

    fn render_profiles_settings(&self) -> Div {
        let profiles = [
            (
                "⚡",
                "PowerShell",
                "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
                true,
            ),
            (
                "▶",
                "Command Prompt",
                "C:\\Windows\\System32\\cmd.exe",
                false,
            ),
            ("🐧", "Ubuntu (WSL)", "\\\\wsl$\\Ubuntu", false),
            (
                "🔀",
                "Git Bash",
                "C:\\Program Files\\Git\\bin\\bash.exe",
                false,
            ),
        ];

        let mut rows: Vec<AnyElement> = Vec::new();
        for (icon, name, path, active) in profiles {
            let mut row = div()
                .w_full()
                .px(px(14.0))
                .py(px(12.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(rgba(0xffffff10))
                .flex()
                .items_center()
                .gap(px(12.0));

            if active {
                row = row.border_color(rgb(ACCENT)).bg(rgba(0x0A84FF14));
            }

            rows.push(
                row.child(
                    div()
                        .w(px(28.0))
                        .h(px(28.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(18.0))
                        .child(icon),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(13.0))
                                .text_color(rgb(TEXT))
                                .child(name),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(11.0))
                                .text_color(rgb(MUTED))
                                .child(path),
                        ),
                )
                .child(if active {
                    div()
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded_full()
                        .bg(rgba(0x0A84FF16))
                        .font_family(UI_FONT)
                        .text_size(px(10.0))
                        .text_color(rgb(ACCENT))
                        .child("既定")
                        .into_any_element()
                } else {
                    div().into_any_element()
                })
                .child(div().text_size(px(13.0)).text_color(rgb(MUTED)).child(">"))
                .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(settings_section_heading("シェルプロファイル"))
            .children(rows)
            .child(
                div()
                    .w_full()
                    .py(px(10.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(rgba(0xffffff20))
                    .font_family(UI_FONT)
                    .text_size(px(13.0))
                    .text_color(rgb(ACCENT))
                    .text_center()
                    .child("+ 新しいプロファイルを追加"),
            )
    }

    fn render_keyboard_settings(&self) -> Div {
        let shortcuts = [
            ("新しいタブ", "Ctrl Shift T"),
            ("タブを閉じる", "Ctrl Shift W"),
            ("ペインを左右分割", "Ctrl Shift D"),
            ("ペインを上下分割", "Ctrl Shift E"),
            ("ペインを閉じる", "Ctrl Shift X"),
            ("次のタブ", "Ctrl Tab"),
            ("前のタブ", "Ctrl Shift Tab"),
            ("設定", "Ctrl Comma"),
        ];

        let mut rows: Vec<AnyElement> = Vec::new();
        for (idx, (action, keys)) in shortcuts.iter().enumerate() {
            let bg = if idx % 2 == 0 {
                rgba(0xffffff06)
            } else {
                rgba(0x00000000)
            };
            rows.push(
                div()
                    .w_full()
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(8.0))
                    .bg(bg)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_family(UI_FONT)
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT_SOFT))
                            .child(*action),
                    )
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .bg(rgba(0xffffff10))
                            .border_1()
                            .border_color(rgba(0xffffff10))
                            .font_family("JetBrains Mono")
                            .text_size(px(12.0))
                            .text_color(rgb(SUBTEXT0))
                            .child(*keys),
                    )
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(settings_section_heading("キーボードショートカット"))
            .children(rows)
    }

    fn render_notifications_settings(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("通知"))
            .child(settings_row("ベル音", toggle(false)))
            .child(settings_row("ビジュアルベル", toggle(true)))
            .child(settings_row("処理完了アラート", toggle(true)))
    }

    fn render_privacy_settings(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("プライバシーとセキュリティ"))
            .child(settings_row("選択時にコピー", toggle(true)))
            .child(settings_row("終了時に履歴を消去", toggle(false)))
            .child(settings_row("テレメトリを送信", toggle(false)))
    }

    fn render_advanced_settings(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("詳細設定"))
            .child(settings_row("GPU アクセラレーション", toggle(true)))
            .child(settings_row(
                "テキストレンダリング",
                select_box("LCD", 120.0),
            ))
            .child(settings_row(
                "単語区切り文字",
                input_box("./\\()\"'-:,.;<>", 180.0, true),
            ))
            .child(settings_row("実験的機能を有効化", toggle(false)))
            .child(section_divider())
            .child(
                div()
                    .px(px(14.0))
                    .py(px(10.0))
                    .rounded(px(10.0))
                    .bg(rgba(0xFF453A1A))
                    .font_family(UI_FONT)
                    .text_size(px(13.0))
                    .text_color(rgb(0xFF453A))
                    .child("すべての設定を初期値に戻す"),
            )
    }

    fn render_status_bar(
        shell_name: &str,
        pane_count: usize,
        tab_count: usize,
        grid_label: &str,
    ) -> impl IntoElement {
        let mut bar = div()
            .id("status-bar")
            .h(px(22.0))
            .w_full()
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .bg(rgba(0x2C2C2ECC))
            .border_t_1()
            .border_color(rgba(0xffffff08))
            .font_family(UI_FONT)
            .text_size(px(10.0))
            .text_color(rgb(SURFACE2))
            .child(shell_name.to_string())
            .child(status_separator())
            .child("UTF-8")
            .child(status_separator())
            .child(grid_label.to_string());

        if pane_count > 1 {
            bar = bar
                .child(status_separator())
                .child(format!("{}P", pane_count));
        }

        bar.child(div().flex_1()).child(format!(
            "{} tab{}",
            tab_count,
            if tab_count == 1 { "" } else { "s" }
        ))
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = window.bounds();
        let new_state = WindowState {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized: window.is_maximized(),
        };

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
        self.last_bounds = Some(new_state.clone());

        if self.bounds_dirty
            && self.last_save_time.elapsed().as_secs() >= WINDOW_STATE_SAVE_INTERVAL_SECS
        {
            if let Err(err) = new_state.save() {
                log::warn!("Failed to save window state: {}", err);
            }
            self.bounds_dirty = false;
            self.last_save_time = Instant::now();
        }

        let state = self.state.read(cx);
        let active_tab = state.active_tab;
        let tab_count = state.tabs.len();
        let active_split = state.tabs.get(active_tab).map(|tab| tab.split.clone());
        let pane_count = active_split
            .as_ref()
            .map(|split| split.read(cx).all_terminals().len())
            .unwrap_or(1);
        let active_shell_name = state
            .tabs
            .get(active_tab)
            .map(|tab| tab.title.clone())
            .unwrap_or_else(|| "PowerShell".to_string());
        let config = state.config.clone();
        let tab_bar_visible = config.tab_bar_visible;
        let grid_label = format!(
            "{}x{}",
            config.default_window_cols, config.default_window_rows
        );
        let tab_infos: Vec<(usize, String, ShellType, bool)> = state
            .tabs
            .iter()
            .enumerate()
            .map(|(idx, tab)| {
                (
                    idx,
                    tab.title.clone(),
                    tab.shell_type.clone(),
                    idx == active_tab,
                )
            })
            .collect();
        let available_shells = state.available_shells.clone();
        let _ = state;

        let mut tab_elements: Vec<AnyElement> = Vec::new();
        for (idx, title, shell_type, is_active) in tab_infos {
            let icon = shell_icon(&shell_type);

            let mut tab = div()
                .id(ElementId::Name(format!("tab-{idx}").into()))
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(6.0))
                .font_family(UI_FONT)
                .text_size(px(12.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        this.state.update(cx, |state, _cx| {
                            state.active_tab = idx;
                        });
                        cx.notify();
                    }),
                );

            if is_active {
                tab = tab.bg(rgba(0xffffff12)).text_color(rgb(TEXT));
            } else {
                tab = tab
                    .text_color(rgb(SUBTEXT1))
                    .hover(|style| style.bg(rgba(0xffffff08)).text_color(rgb(SUBTEXT0)));
            }

            tab = tab
                .child(div().w(px(10.0)).text_size(px(10.0)).child(icon))
                .child(title);

            if tab_count > 1 {
                tab = tab.child(
                    div()
                        .id(ElementId::Name(format!("tab-close-{idx}").into()))
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded(px(4.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_size(px(10.0))
                        .text_color(rgb(if is_active { SUBTEXT1 } else { MUTED }))
                        .hover(|style| style.bg(rgba(0xffffff12)).text_color(rgb(TEXT)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                this.state.update(cx, |state, _cx| {
                                    state.close_tab(idx);
                                });
                                cx.notify();
                            }),
                        )
                        .child("x"),
                );
            }

            tab_elements.push(tab.into_any_element());
        }

        let titlebar_actions = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(chrome_button("title-add", "ui/plus.svg").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_settings = false;
                    this.show_shell_menu = true;
                    cx.notify();
                }),
            ))
            .child(
                chrome_button("title-shells", "ui/chevron-down.svg").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.show_settings = false;
                        this.show_shell_menu = true;
                        cx.notify();
                    }),
                ),
            )
            .child(
                chrome_button("title-settings", "ui/settings.svg").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.show_shell_menu = false;
                        this.show_settings = true;
                        cx.notify();
                    }),
                ),
            );

        let title_bar = div()
            .id("title-bar")
            .h(px(38.0))
            .w_full()
            .px(px(12.0))
            .border_b_1()
            .border_color(rgba(0xffffff08))
            .bg(rgb(TITLEBAR_BG))
            .flex()
            .items_center()
            .child(self.render_window_traffic_lights(window, cx))
            .child(
                div()
                    .flex_1()
                    .mx(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .children(if tab_bar_visible {
                        tab_elements
                    } else {
                        vec![
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(12.0))
                                .text_color(rgb(SUBTEXT0))
                                .child(active_shell_name.clone())
                                .into_any_element(),
                        ]
                    }),
            )
            .child(titlebar_actions);

        let shell_backdrop = if self.show_shell_menu {
            Some(
                div()
                    .id("shell-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(rgba(BACKDROP)),
            )
        } else {
            None
        };

        let settings_backdrop = if self.show_settings {
            Some(
                div()
                    .id("settings-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(rgba(BACKDROP)),
            )
        } else {
            None
        };

        let close_confirm_backdrop = if self.show_close_confirm {
            Some(
                div()
                    .id("close-confirm-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(rgba(BACKDROP)),
            )
        } else {
            None
        };

        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(WINDOW_BG))
            .font_family(UI_FONT)
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_split_right))
            .on_action(cx.listener(Self::on_split_down))
            .on_action(cx.listener(Self::on_close_pane))
            .on_action(cx.listener(Self::on_focus_next))
            .on_action(cx.listener(Self::on_focus_prev))
            .on_action(cx.listener(Self::on_quit_requested))
            .child(title_bar)
            .child(
                div()
                    .id("terminal-area")
                    .flex_1()
                    .overflow_hidden()
                    .bg(rgb(WINDOW_BG))
                    .children(active_split),
            )
            .child(Self::render_status_bar(
                &active_shell_name,
                pane_count,
                tab_count,
                &grid_label,
            ))
            .children(shell_backdrop)
            .children(self.render_shell_selector(
                new_state.width,
                new_state.height,
                &available_shells,
                cx,
            ))
            .children(settings_backdrop)
            .children(self.render_settings_panel(new_state.width, new_state.height, window, cx))
            .children(close_confirm_backdrop)
            .children(self.render_close_confirm_dialog(new_state.width, new_state.height, cx))
    }
}

impl Drop for RootView {
    fn drop(&mut self) {
        if let Some(ref state) = self.last_bounds {
            if let Err(err) = state.save() {
                log::warn!("Failed to save window state: {}", err);
            }
        }
    }
}

fn terminal_settings_from_config(config: &AppConfig) -> TerminalSettings {
    TerminalSettings {
        cols: config.default_window_cols,
        rows: config.default_window_rows,
        scrollback_lines: config.scrollback_lines,
    }
}

fn resolve_default_shell_command(config_shell: &str, available_shells: &[ShellEntry]) -> String {
    available_shells
        .iter()
        .find(|entry| entry.command == config_shell)
        .map(|entry| entry.command.clone())
        .or_else(|| available_shells.first().map(|entry| entry.command.clone()))
        .unwrap_or_else(shell::detect_default_shell)
}

fn window_size_from_grid(cols: u16, rows: u16) -> Size<Pixels> {
    let width = ((cols as f32 * CELL_WIDTH_ESTIMATE) + 48.0).clamp(400.0, 2400.0);
    let height = ((rows as f32 * CELL_HEIGHT_ESTIMATE) + WINDOW_CHROME_HEIGHT).clamp(300.0, 1600.0);
    size(px(width), px(height))
}

fn shell_meta_for_command(command: &str, available_shells: &[ShellEntry]) -> (ShellType, String) {
    if let Some(entry) = available_shells
        .iter()
        .find(|entry| entry.command == command)
    {
        return (entry.shell_type.clone(), entry.display_name.clone());
    }

    let lower = command.to_lowercase();
    if lower.contains("pwsh") {
        (
            ShellType::Pwsh,
            shell_display_name(&ShellType::Pwsh).to_string(),
        )
    } else if lower.contains("powershell") {
        (
            ShellType::PowerShell,
            shell_display_name(&ShellType::PowerShell).to_string(),
        )
    } else if lower.contains("wsl") {
        (
            ShellType::Wsl,
            shell_display_name(&ShellType::Wsl).to_string(),
        )
    } else if lower.contains("bash") {
        (
            ShellType::GitBash,
            shell_display_name(&ShellType::GitBash).to_string(),
        )
    } else {
        (
            ShellType::Cmd,
            shell_display_name(&ShellType::Cmd).to_string(),
        )
    }
}

fn shell_display_name(shell_type: &ShellType) -> &'static str {
    match shell_type {
        ShellType::PowerShell => "Windows PowerShell",
        ShellType::Pwsh => "PowerShell",
        ShellType::Cmd => "Command Prompt",
        ShellType::Wsl => "Ubuntu (WSL)",
        ShellType::GitBash => "Git Bash",
    }
}

fn shell_description(shell_type: &ShellType) -> &'static str {
    match shell_type {
        ShellType::PowerShell => "Windows PowerShell 5.1",
        ShellType::Pwsh => "Windows PowerShell 7.x",
        ShellType::Cmd => "Windows Command Processor",
        ShellType::Wsl => "Windows Subsystem for Linux",
        ShellType::GitBash => "Git for Windows Bash",
    }
}

fn shell_icon(shell_type: &ShellType) -> &'static str {
    match shell_type {
        ShellType::PowerShell | ShellType::Pwsh => "⚡",
        ShellType::Cmd => "▶",
        ShellType::Wsl => "🐧",
        ShellType::GitBash => "🔀",
    }
}

fn chrome_button(id: &'static str, icon_path: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(24.0))
        .h(px(24.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .font_family(UI_FONT)
        .text_size(px(12.0))
        .hover(|style| style.bg(rgba(0xffffff10)))
        .child(
            svg()
                .path(icon_path)
                .size(px(14.0))
                .text_color(rgb(SUBTEXT1)),
        )
}

fn settings_section_heading(label: &'static str) -> Div {
    div()
        .font_family(UI_FONT)
        .text_size(px(14.0))
        .text_color(rgb(TEXT))
        .child(label)
}

fn section_divider() -> Div {
    div().w_full().h(px(1.0)).bg(rgba(0xffffff10))
}

fn status_separator() -> Div {
    div().text_color(rgb(SURFACE1)).child("|")
}

fn settings_row(label: &'static str, control: impl IntoElement) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(TEXT_SOFT))
                .child(label),
        )
        .child(control)
}

fn select_box(value: &'static str, width: f32) -> Div {
    div()
        .w(px(width))
        .h(px(32.0))
        .rounded(px(8.0))
        .bg(rgba(0xffffff10))
        .border_1()
        .border_color(rgba(0xffffff10))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(TEXT))
                .child(value),
        )
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(10.0))
                .text_color(rgb(SUBTEXT1))
                .child("v"),
        )
}

fn interactive_select_box(
    value: String,
    width: f32,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(width))
        .h(px(32.0))
        .rounded(px(8.0))
        .bg(rgba(0xffffff10))
        .border_1()
        .border_color(rgba(0xffffff10))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_between()
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xffffff16)))
        .on_mouse_down(MouseButton::Left, listener)
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(TEXT))
                .child(value),
        )
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(10.0))
                .text_color(rgb(SUBTEXT1))
                .child("v"),
        )
}

fn input_box(value: &'static str, width: f32, mono: bool) -> Div {
    let mut input = div()
        .w(px(width))
        .h(px(32.0))
        .rounded(px(8.0))
        .bg(rgba(0xffffff10))
        .border_1()
        .border_color(rgba(0xffffff10))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .text_color(rgb(TEXT))
        .child(value);

    if mono {
        input = input.font_family("JetBrains Mono");
    } else {
        input = input.font_family(UI_FONT);
    }

    input
}

fn input_box_dynamic(value: String, width: f32, mono: bool) -> Div {
    let mut input = div()
        .w(px(width))
        .h(px(32.0))
        .rounded(px(8.0))
        .bg(rgba(0xffffff10))
        .border_1()
        .border_color(rgba(0xffffff10))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .text_color(rgb(TEXT))
        .child(value);

    if mono {
        input = input.font_family("JetBrains Mono");
    } else {
        input = input.font_family(UI_FONT);
    }

    input
}

fn toggle(on: bool) -> Div {
    let mut root = div()
        .w(px(38.0))
        .h(px(22.0))
        .rounded_full()
        .bg(rgb(if on { ACCENT_ALT } else { MUTED }))
        .px(px(2.0))
        .flex()
        .items_center();

    if on {
        root = root.justify_end();
    }

    root.child(
        div()
            .w(px(18.0))
            .h(px(18.0))
            .rounded_full()
            .bg(rgb(0xffffff)),
    )
}

fn interactive_toggle(
    on: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    toggle(on)
        .cursor_pointer()
        .hover(|style| style.opacity(0.92))
        .on_mouse_down(MouseButton::Left, listener)
}

fn stepper_button(
    label: &'static str,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(22.0))
        .h(px(22.0))
        .rounded(px(6.0))
        .bg(rgba(0xffffff10))
        .border_1()
        .border_color(rgba(0xffffff10))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xffffff16)))
        .flex()
        .items_center()
        .justify_center()
        .font_family(UI_FONT)
        .text_size(px(12.0))
        .text_color(rgb(TEXT))
        .on_mouse_down(MouseButton::Left, listener)
        .child(label)
}

fn number_stepper(
    value: String,
    width: f32,
    mono: bool,
    on_decrement: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_increment: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(stepper_button("-", on_decrement))
        .child(input_box_dynamic(value, width, mono))
        .child(stepper_button("+", on_increment))
}

fn interactive_action_button(
    label: &'static str,
    accent: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let mut button = div()
        .h(px(32.0))
        .px(px(14.0))
        .rounded(px(8.0))
        .border_1()
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .font_family(UI_FONT)
        .text_size(px(13.0))
        .on_mouse_down(MouseButton::Left, listener);

    if accent {
        button = button
            .bg(rgb(ACCENT))
            .border_color(rgb(ACCENT))
            .text_color(rgb(0xffffff))
            .hover(|style| style.bg(rgb(0x409CFF)));
    } else {
        button = button
            .bg(rgba(0xffffff10))
            .border_color(rgba(0xffffff10))
            .text_color(rgb(TEXT))
            .hover(|style| style.bg(rgba(0xffffff16)));
    }

    button.child(label)
}

fn slider_with_value(fill_ratio: f32, value: &'static str) -> Div {
    let fill_width = 112.0 * fill_ratio.clamp(0.0, 1.0);

    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(
            div()
                .w(px(112.0))
                .h(px(4.0))
                .rounded_full()
                .bg(rgba(0xffffff12))
                .child(
                    div()
                        .w(px(fill_width))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(rgb(ACCENT)),
                ),
        )
        .child(
            div()
                .w(px(40.0))
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(SUBTEXT1))
                .text_right()
                .child(value),
        )
}

fn segmented_control(items: &[(&'static str, bool)]) -> Div {
    let mut children = Vec::new();
    for (label, active) in items {
        let mut item = div()
            .px(px(12.0))
            .py(px(6.0))
            .rounded(px(7.0))
            .font_family(UI_FONT)
            .text_size(px(12.0));

        if *active {
            item = item.bg(rgba(0xffffff14)).text_color(rgb(TEXT));
        } else {
            item = item.text_color(rgb(SUBTEXT1));
        }

        children.push(item.child(*label).into_any_element());
    }

    div()
        .rounded(px(8.0))
        .bg(rgba(0xffffff08))
        .p(px(2.0))
        .flex()
        .items_center()
        .gap(px(2.0))
        .children(children)
}

fn theme_card(name: &'static str, preview: u32, selected: bool) -> Div {
    let mut card = div()
        .w(px(144.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(rgba(0xffffff10))
        .bg(rgba(0xffffff04))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(8.0));

    if selected {
        card = card.border_color(rgb(ACCENT)).bg(rgba(0x0A84FF10));
    }

    card.child(
        div()
            .w_full()
            .h(px(46.0))
            .rounded(px(8.0))
            .bg(rgb(preview))
            .p(px(6.0))
            .flex()
            .items_end()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .child(color_dot(RED))
                    .child(color_dot(YELLOW))
                    .child(color_dot(GREEN)),
            ),
    )
    .child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(12.0))
                    .text_color(rgb(TEXT_SOFT))
                    .child(name),
            )
            .child(if selected {
                div()
                    .font_family(UI_FONT)
                    .text_size(px(12.0))
                    .text_color(rgb(ACCENT))
                    .child("o")
                    .into_any_element()
            } else {
                div().into_any_element()
            }),
    )
}

fn color_dot(color: u32) -> Div {
    div().w(px(6.0)).h(px(6.0)).rounded_full().bg(rgb(color))
}
