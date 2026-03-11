//! Application state and root view — Figma-aligned macOS terminal chrome

use std::time::Instant;

use gpui::*;

use crate::config::WindowState;
use crate::shell::{self, ShellType};
use crate::split::{FocusDir, SplitContainer, SplitDirection};
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
    ("Dark", WINDOW_BG),
    ("Light", 0xF5F5F7),
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
            (Self::General, "General", "◻"),
            (Self::Appearance, "Appearance", "◌"),
            (Self::Profiles, "Profiles", ">_"),
            (Self::Keyboard, "Keyboard", "⌨"),
            (Self::Notifications, "Notifications", "♪"),
            (Self::Privacy, "Privacy", "⌂"),
            (Self::Advanced, "Advanced", "⚙"),
        ]
    }

    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Profiles => "Profiles",
            Self::Keyboard => "Keyboard",
            Self::Notifications => "Notifications",
            Self::Privacy => "Privacy",
            Self::Advanced => "Advanced",
        }
    }
}

/// Root view containing tab bar + split container + overlays.
pub struct RootView {
    state: Entity<AppState>,
    show_shell_menu: bool,
    show_settings: bool,
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

        let default_shell = available_shells
            .first()
            .map(|entry| entry.command.clone())
            .unwrap_or_else(shell::detect_default_shell);
        let (shell_type, title) = shell_meta_for_command(&default_shell, &available_shells);
        let split = cx.new(|cx| SplitContainer::new(&default_shell, cx));

        Self {
            tabs: vec![Tab {
                title,
                shell_type,
                split,
            }],
            active_tab: 0,
            available_shells,
        }
    }

    pub fn add_tab(&mut self, cx: &mut App) {
        let shell = self
            .available_shells
            .first()
            .map(|entry| entry.command.clone())
            .unwrap_or_else(shell::detect_default_shell);
        self.add_tab_with_shell(&shell, cx);
    }

    pub fn add_tab_with_shell(&mut self, shell: &str, cx: &mut App) {
        let (shell_type, title) = shell_meta_for_command(shell, &self.available_shells);
        let split = cx.new(|cx| SplitContainer::new(shell, cx));

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
}

impl RootView {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            show_shell_menu: false,
            show_settings: false,
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

    fn render_window_traffic_lights(&mut self, cx: &mut Context<Self>) -> Div {
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
                        cx.listener(|_this, _: &MouseDownEvent, _window, cx| cx.quit()),
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
                        .child(div().flex().flex_col().gap(px(2.0)).children(shell_items))
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
                                .children(Some(self.render_settings_content())),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_settings_content(&self) -> AnyElement {
        match self.settings_category {
            SettingsCategory::General => self.render_general_settings().into_any_element(),
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

    fn render_general_settings(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(24.0))
            .child(settings_section_heading("Startup"))
            .child(settings_row(
                "Default Profile",
                select_box("PowerShell", 150.0),
            ))
            .child(settings_row("Launch on Login", toggle(true)))
            .child(settings_row("Show Tab Bar", toggle(true)))
            .child(settings_row("Confirm on Close", toggle(true)))
            .child(section_divider())
            .child(settings_section_heading("Window"))
            .child(settings_row(
                "New Window Size",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(input_box("120", 64.0, false))
                    .child(
                        div()
                            .font_family(UI_FONT)
                            .text_size(px(13.0))
                            .text_color(rgb(SUBTEXT1))
                            .child("x"),
                    )
                    .child(input_box("30", 64.0, false)),
            ))
            .child(settings_row(
                "Scrollback Lines",
                input_box("10000", 96.0, false),
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
            .child(settings_section_heading("Theme"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(div().flex().gap(px(12.0)).children(theme_rows_top))
                    .child(div().flex().gap(px(12.0)).children(theme_rows_bottom)),
            )
            .child(section_divider())
            .child(settings_section_heading("Font"))
            .child(settings_row(
                "Font Family",
                select_box("JetBrains Mono", 180.0),
            ))
            .child(settings_row("Font Size", slider_with_value(0.45, "13px")))
            .child(section_divider())
            .child(settings_section_heading("Cursor"))
            .child(settings_row(
                "Cursor Style",
                segmented_control(&[("Bar", true), ("Block", false), ("Underline", false)]),
            ))
            .child(settings_row("Cursor Blink", toggle(true)))
            .child(section_divider())
            .child(settings_section_heading("Transparency"))
            .child(settings_row(
                "Window Opacity",
                slider_with_value(0.95, "95%"),
            ))
            .child(settings_row(
                "Background Blur",
                slider_with_value(0.33, "10px"),
            ))
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
                        .child("Default")
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
            .child(settings_section_heading("Shell Profiles"))
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
                    .child("+ Add New Profile"),
            )
    }

    fn render_keyboard_settings(&self) -> Div {
        let shortcuts = [
            ("New Tab", "Ctrl Shift T"),
            ("Close Tab", "Ctrl Shift W"),
            ("Split Pane Horizontal", "Ctrl Shift D"),
            ("Split Pane Vertical", "Ctrl Shift E"),
            ("Close Pane", "Ctrl Shift X"),
            ("Next Tab", "Ctrl Tab"),
            ("Previous Tab", "Ctrl Shift Tab"),
            ("Settings", "Ctrl Comma"),
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
            .child(settings_section_heading("Keyboard Shortcuts"))
            .children(rows)
    }

    fn render_notifications_settings(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("Notifications"))
            .child(settings_row("Bell Sound", toggle(false)))
            .child(settings_row("Visual Bell", toggle(true)))
            .child(settings_row("Process Completion Alerts", toggle(true)))
    }

    fn render_privacy_settings(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("Privacy & Security"))
            .child(settings_row("Copy on Select", toggle(true)))
            .child(settings_row("Clear History on Close", toggle(false)))
            .child(settings_row("Send Telemetry", toggle(false)))
    }

    fn render_advanced_settings(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("Advanced Settings"))
            .child(settings_row("GPU Acceleration", toggle(true)))
            .child(settings_row("Text Rendering", select_box("LCD", 120.0)))
            .child(settings_row(
                "Word Separators",
                input_box("./\\()\"'-:,.;<>", 180.0, true),
            ))
            .child(settings_row("Enable Experimental Features", toggle(false)))
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
                    .child("Reset All Settings to Default"),
            )
    }

    fn render_status_bar(
        shell_name: &str,
        pane_count: usize,
        tab_count: usize,
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
            .child("120x30");

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
            .child(chrome_button("title-add", "+").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_settings = false;
                    this.show_shell_menu = true;
                    cx.notify();
                }),
            ))
            .child(chrome_button("title-shells", "v").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_settings = false;
                    this.show_shell_menu = true;
                    cx.notify();
                }),
            ))
            .child(chrome_button("title-settings", "o").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_shell_menu = false;
                    this.show_settings = true;
                    cx.notify();
                }),
            ));

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
            .child(self.render_window_traffic_lights(cx))
            .child(
                div()
                    .flex_1()
                    .mx(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .children(tab_elements),
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
                    .bg(rgba(BACKDROP))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.show_shell_menu = false;
                            cx.notify();
                        }),
                    ),
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
                    .bg(rgba(BACKDROP))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.show_settings = false;
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
            ))
            .children(shell_backdrop)
            .children(self.render_shell_selector(
                new_state.width,
                new_state.height,
                &available_shells,
                cx,
            ))
            .children(settings_backdrop)
            .children(self.render_settings_panel(new_state.width, new_state.height, cx))
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

fn chrome_button(id: &'static str, label: &'static str) -> Stateful<Div> {
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
        .text_color(rgb(SUBTEXT1))
        .hover(|style| style.bg(rgba(0xffffff10)).text_color(rgb(TEXT)))
        .child(label)
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
