//! Application state and root view — Figma-aligned macOS terminal chrome

use std::time::Instant;

use gpui::*;

use crate::config::{AppConfig, WindowState, set_launch_on_login};
use crate::shell::{self, ShellType};
use crate::snippets::{CsvEncoding, SnippetPalette, SnippetQueueMode};
use crate::split::{FocusDir, SplitContainer, SplitDirection};
use crate::terminal::TerminalSettings;
use crate::terminal::view::{CELL_HEIGHT_ESTIMATE, CELL_WIDTH_ESTIMATE, WINDOW_CHROME_HEIGHT};
use crate::{
    ClosePane, CloseTab, FocusNext, FocusPrev, NewTab, SnippetQueuePaste, SplitDown, SplitRight,
    ToggleSnippetPalette,
};

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnippetGroupEditMode {
    None,
    Rename(usize),
    New,
}

#[derive(Clone)]
struct SnippetGroupEditorState {
    selected_index: usize,
    edit_mode: SnippetGroupEditMode,
    draft_name: String,
}

impl Default for SnippetGroupEditorState {
    fn default() -> Self {
        Self {
            selected_index: 0,
            edit_mode: SnippetGroupEditMode::None,
            draft_name: String::new(),
        }
    }
}

#[derive(Clone, Default)]
struct SnippetListEditorState {
    selected_group_index: usize,
    selected_item_store_index: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnippetEditorMode {
    Add,
    Edit(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnippetEditorField {
    Title,
    Content,
}

#[derive(Clone)]
struct SnippetEditorState {
    mode: SnippetEditorMode,
    title: String,
    content: String,
    group_index: usize,
    active_field: SnippetEditorField,
}

#[derive(Clone)]
struct SnippetCsvDialogState {
    export_encoding: CsvEncoding,
    import_encoding: CsvEncoding,
    clear_before_import: bool,
}

impl Default for SnippetCsvDialogState {
    fn default() -> Self {
        Self {
            export_encoding: CsvEncoding::ShiftJis,
            import_encoding: CsvEncoding::ShiftJis,
            clear_before_import: false,
        }
    }
}

/// Root view containing tab bar + split container + overlays.
pub struct RootView {
    state: Entity<AppState>,
    focus_handle: FocusHandle,
    show_shell_menu: bool,
    show_settings: bool,
    show_close_confirm: bool,
    show_snippet_context_menu: bool,
    snippet_palette: SnippetPalette,
    snippet_group_editor: Option<SnippetGroupEditorState>,
    snippet_list_editor: Option<SnippetListEditorState>,
    snippet_editor: Option<SnippetEditorState>,
    snippet_csv_dialog: Option<SnippetCsvDialogState>,
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
            focus_handle: _cx.focus_handle(),
            show_shell_menu: false,
            show_settings: false,
            show_close_confirm: false,
            show_snippet_context_menu: false,
            snippet_palette: SnippetPalette::load(),
            snippet_group_editor: None,
            snippet_list_editor: None,
            snippet_editor: None,
            snippet_csv_dialog: None,
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

    fn on_toggle_snippet_palette(
        &mut self,
        _action: &ToggleSnippetPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_shell_menu = false;
        self.show_settings = false;
        self.show_close_confirm = false;
        self.show_snippet_context_menu = false;
        self.snippet_palette.toggle();
        if self.snippet_palette.is_visible() {
            self.focus_handle.focus(window);
        } else {
            self.focus_active_terminal(window, cx);
        }
        cx.notify();
    }

    fn on_snippet_queue_paste(
        &mut self,
        _action: &SnippetQueuePaste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_next_queued_snippet(window, cx);
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
            self.show_snippet_context_menu = false;
            self.snippet_palette.hide();
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

    fn focus_active_terminal(&self, window: &mut Window, cx: &Context<Self>) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            if let Some(terminal) = split.read(cx).focused_terminal() {
                terminal.read(cx).focus_handle(cx).focus(window);
            }
        }
    }

    fn dispatch_snippet_to_active_terminal(
        &mut self,
        content: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            if let Some(terminal) = split.read(cx).focused_terminal() {
                let _ = terminal.read(cx).send_input(content.as_bytes());
            }
        }

        self.snippet_palette.hide();
        self.focus_active_terminal(window, cx);
        cx.notify();
    }

    fn activate_selected_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.snippet_palette.activate_selected() {
            Some(content) => self.dispatch_snippet_to_active_terminal(content, window, cx),
            None => cx.notify(),
        }
    }

    fn paste_next_queued_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(content) = self.snippet_palette.dequeue() else {
            return;
        };

        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            if let Some(terminal) = split.read(cx).focused_terminal() {
                let _ = terminal.read(cx).send_input(content.as_bytes());
            }
        }

        self.focus_active_terminal(window, cx);
        cx.notify();
    }

    fn close_snippet_dialogs(&mut self) {
        self.snippet_group_editor = None;
        self.snippet_list_editor = None;
        self.snippet_editor = None;
        self.snippet_csv_dialog = None;
    }

    fn open_snippet_group_editor(&mut self) {
        self.close_snippet_dialogs();
        self.snippet_palette.hide();
        let selected_index = self
            .snippet_palette
            .active_group()
            .and_then(|group| self.snippet_palette.group_index_by_name(group))
            .unwrap_or(0);
        self.snippet_group_editor = Some(SnippetGroupEditorState {
            selected_index,
            ..SnippetGroupEditorState::default()
        });
    }

    fn open_snippet_list_editor(&mut self) {
        self.close_snippet_dialogs();
        self.snippet_palette.hide();
        let selected_group_index = self
            .snippet_palette
            .active_group()
            .and_then(|group| self.snippet_palette.group_index_by_name(group))
            .unwrap_or(0);
        let selected_item_store_index = self
            .snippet_palette
            .group_name_at(selected_group_index)
            .and_then(|group| {
                self.snippet_palette
                    .snippets_for_group(Some(group))
                    .first()
                    .map(|(store_index, _)| *store_index)
            });
        self.snippet_list_editor = Some(SnippetListEditorState {
            selected_group_index,
            selected_item_store_index,
        });
    }

    fn open_snippet_editor(&mut self, mode: SnippetEditorMode, group_hint: Option<usize>) {
        let state = match mode {
            SnippetEditorMode::Add => SnippetEditorState {
                mode,
                title: String::new(),
                content: String::new(),
                group_index: group_hint.unwrap_or(0),
                active_field: SnippetEditorField::Title,
            },
            SnippetEditorMode::Edit(store_index) => {
                let snippet = self
                    .snippet_palette
                    .store()
                    .items()
                    .get(store_index)
                    .cloned()
                    .unwrap_or_else(|| crate::snippets::Snippet::new("", ""));
                let group_index = self
                    .snippet_palette
                    .group_index_by_name(&snippet.group)
                    .unwrap_or(0);
                SnippetEditorState {
                    mode,
                    title: snippet.title,
                    content: snippet.content,
                    group_index,
                    active_field: SnippetEditorField::Title,
                }
            }
        };

        self.snippet_editor = Some(state);
    }

    fn open_snippet_csv_dialog(&mut self) {
        self.close_snippet_dialogs();
        self.snippet_palette.hide();
        self.snippet_csv_dialog = Some(SnippetCsvDialogState::default());
    }

    fn close_snippet_group_editor(&mut self) {
        self.snippet_group_editor = None;
    }

    fn close_snippet_list_editor(&mut self) {
        self.snippet_list_editor = None;
    }

    fn close_snippet_editor(&mut self) {
        self.snippet_editor = None;
    }

    fn close_snippet_csv_dialog(&mut self) {
        self.snippet_csv_dialog = None;
    }

    fn sync_list_editor_selection(&mut self) {
        let Some(state) = self.snippet_list_editor.as_mut() else {
            return;
        };
        let Some(group_name) = self
            .snippet_palette
            .group_name_at(state.selected_group_index)
        else {
            state.selected_group_index = 0;
            state.selected_item_store_index =
                self.snippet_palette.group_name_at(0).and_then(|group| {
                    self.snippet_palette
                        .snippets_for_group(Some(group))
                        .first()
                        .map(|(store_index, _)| *store_index)
                });
            return;
        };

        let group_items = self.snippet_palette.snippets_for_group(Some(group_name));
        if group_items.is_empty() {
            state.selected_item_store_index = None;
            return;
        }

        if !state
            .selected_item_store_index
            .map(|selected| {
                group_items
                    .iter()
                    .any(|(store_index, _)| *store_index == selected)
            })
            .unwrap_or(false)
        {
            state.selected_item_store_index = Some(group_items[0].0);
        }
    }

    fn apply_group_editor_edit(&mut self) {
        let Some(editor) = self.snippet_group_editor.as_mut() else {
            return;
        };
        let draft_name = editor.draft_name.trim().to_string();
        if draft_name.is_empty() {
            editor.edit_mode = SnippetGroupEditMode::None;
            editor.draft_name.clear();
            return;
        }

        let updated = match editor.edit_mode {
            SnippetGroupEditMode::Rename(index) => {
                self.snippet_palette.rename_group(index, draft_name)
            }
            SnippetGroupEditMode::New => self.snippet_palette.add_group_named(draft_name),
            SnippetGroupEditMode::None => false,
        };

        if updated {
            editor.selected_index = match editor.edit_mode {
                SnippetGroupEditMode::Rename(index) => index,
                SnippetGroupEditMode::New => self.snippet_palette.groups().len().saturating_sub(1),
                SnippetGroupEditMode::None => editor.selected_index,
            };
        }

        editor.edit_mode = SnippetGroupEditMode::None;
        editor.draft_name.clear();
    }

    fn save_snippet_editor(&mut self) {
        let Some(editor) = self.snippet_editor.as_ref().cloned() else {
            return;
        };
        let group_name = self
            .snippet_palette
            .group_name_at(editor.group_index)
            .unwrap_or("General")
            .to_string();
        let selected_store_index = match editor.mode {
            SnippetEditorMode::Add => {
                self.snippet_palette
                    .add_snippet(editor.title, editor.content, Some(&group_name))
            }
            SnippetEditorMode::Edit(store_index) => self
                .snippet_palette
                .update_snippet(store_index, editor.title, editor.content, Some(&group_name))
                .then_some(store_index),
        };

        if let Some(selected_store_index) = selected_store_index {
            self.snippet_editor = None;
            if let Some(state) = self.snippet_list_editor.as_mut() {
                state.selected_group_index = editor.group_index;
                state.selected_item_store_index = Some(selected_store_index);
            }
            self.sync_list_editor_selection();
        }
    }

    fn handle_snippet_group_editor_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.snippet_group_editor.as_mut() else {
            return false;
        };

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.close_snippet_group_editor();
                cx.notify();
            }
            "up" => {
                editor.selected_index = editor.selected_index.saturating_sub(1);
                cx.notify();
            }
            "down" => {
                editor.selected_index = (editor.selected_index + 1)
                    .min(self.snippet_palette.groups().len().saturating_sub(1));
                cx.notify();
            }
            "enter" => {
                if editor.edit_mode != SnippetGroupEditMode::None {
                    self.apply_group_editor_edit();
                    cx.notify();
                }
            }
            "backspace" => {
                if editor.edit_mode != SnippetGroupEditMode::None {
                    editor.draft_name.pop();
                    cx.notify();
                }
            }
            _ => {
                if let Some(text) = &event.keystroke.key_char {
                    if !text.is_empty()
                        && !event.keystroke.modifiers.control
                        && editor.edit_mode != SnippetGroupEditMode::None
                    {
                        editor.draft_name.push_str(text);
                        cx.notify();
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }
        true
    }

    fn handle_snippet_list_editor_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.snippet_list_editor.as_ref().cloned() else {
            return false;
        };
        let Some(group_name) = self
            .snippet_palette
            .group_name_at(state.selected_group_index)
            .map(str::to_string)
        else {
            return false;
        };
        let group_items = self.snippet_palette.snippets_for_group(Some(&group_name));
        let selected_position = state
            .selected_item_store_index
            .and_then(|selected| {
                group_items
                    .iter()
                    .position(|(store_index, _)| *store_index == selected)
            })
            .unwrap_or(0);

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.close_snippet_list_editor();
                cx.notify();
            }
            "up" => {
                if let Some((store_index, _)) = group_items.get(selected_position.saturating_sub(1))
                {
                    if let Some(state) = self.snippet_list_editor.as_mut() {
                        state.selected_item_store_index = Some(*store_index);
                        cx.notify();
                    }
                }
            }
            "down" => {
                if let Some((store_index, _)) = group_items
                    .get((selected_position + 1).min(group_items.len().saturating_sub(1)))
                {
                    if let Some(state) = self.snippet_list_editor.as_mut() {
                        state.selected_item_store_index = Some(*store_index);
                        cx.notify();
                    }
                }
            }
            "delete" => {
                if let Some(store_index) = state.selected_item_store_index {
                    if self.snippet_palette.delete_snippet(store_index) {
                        self.sync_list_editor_selection();
                        cx.notify();
                    }
                }
            }
            "enter" => {
                if let Some(store_index) = state.selected_item_store_index {
                    self.open_snippet_editor(SnippetEditorMode::Edit(store_index), None);
                    cx.notify();
                }
            }
            _ => {
                if event.keystroke.modifiers.control && event.keystroke.key == "n" {
                    self.open_snippet_editor(
                        SnippetEditorMode::Add,
                        Some(state.selected_group_index),
                    );
                    cx.notify();
                } else {
                    return false;
                }
            }
        }

        let _ = window;
        true
    }

    fn handle_snippet_editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.snippet_editor.as_mut() else {
            return false;
        };

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.close_snippet_editor();
                cx.notify();
            }
            "tab" => {
                editor.active_field = match editor.active_field {
                    SnippetEditorField::Title => SnippetEditorField::Content,
                    SnippetEditorField::Content => SnippetEditorField::Title,
                };
                cx.notify();
            }
            "enter" if event.keystroke.modifiers.control => {
                self.save_snippet_editor();
                cx.notify();
            }
            "enter" => {
                match editor.active_field {
                    SnippetEditorField::Title => editor.active_field = SnippetEditorField::Content,
                    SnippetEditorField::Content => editor.content.push('\n'),
                }
                cx.notify();
            }
            "backspace" => {
                match editor.active_field {
                    SnippetEditorField::Title => {
                        editor.title.pop();
                    }
                    SnippetEditorField::Content => {
                        editor.content.pop();
                    }
                }
                cx.notify();
            }
            _ => {
                if let Some(text) = &event.keystroke.key_char {
                    if !text.is_empty() && !event.keystroke.modifiers.control {
                        match editor.active_field {
                            SnippetEditorField::Title => editor.title.push_str(text),
                            SnippetEditorField::Content => editor.content.push_str(text),
                        }
                        cx.notify();
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }
        true
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_snippet_editor_key(event, cx)
            || self.handle_snippet_group_editor_key(event, cx)
            || self.handle_snippet_list_editor_key(event, window, cx)
        {
            cx.stop_propagation();
            return;
        }

        if self.snippet_csv_dialog.is_some() {
            if event.keystroke.key == "escape" {
                self.close_snippet_csv_dialog();
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }

        if !self.snippet_palette.is_visible() {
            return;
        }

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.snippet_palette.hide();
                self.focus_active_terminal(window, cx);
                cx.notify();
            }
            "enter" => {
                self.activate_selected_snippet(window, cx);
            }
            "up" => {
                self.snippet_palette.select_previous();
                cx.notify();
            }
            "down" => {
                self.snippet_palette.select_next();
                cx.notify();
            }
            "tab" => {
                self.snippet_palette
                    .cycle_group(!event.keystroke.modifiers.shift);
                cx.notify();
            }
            "delete" => {
                if self.snippet_palette.delete_selected() {
                    cx.notify();
                } else {
                    return;
                }
            }
            "backspace" => {
                let mut query = self.snippet_palette.query().to_string();
                if query.pop().is_some() {
                    self.snippet_palette.set_query(query);
                } else {
                    self.snippet_palette.clear_query();
                }
                cx.notify();
            }
            _ => {
                if event.keystroke.modifiers.control && event.keystroke.key == "n" {
                    if self.snippet_palette.create_snippet_from_query() {
                        cx.notify();
                    }
                    cx.stop_propagation();
                    return;
                }

                if let Some(text) = &event.keystroke.key_char {
                    if !text.is_empty() && !event.keystroke.modifiers.control {
                        let mut query = self.snippet_palette.query().to_string();
                        query.push_str(text);
                        self.snippet_palette.set_query(query);
                        cx.notify();
                    } else {
                        return;
                    }
                } else {
                    return;
                }
            }
        }

        cx.stop_propagation();
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

    fn render_snippet_palette(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.snippet_palette.is_visible() {
            return None;
        }

        let panel_w = (viewport_w - 48.0).clamp(640.0, 860.0);
        let panel_h = (viewport_h - 64.0).clamp(420.0, 600.0);
        let top = ((viewport_h - panel_h) * 0.5).max(24.0);
        let left = ((viewport_w - panel_w) * 0.5).max(24.0);
        let query_is_empty = self.snippet_palette.query().is_empty();
        let display_text = if query_is_empty {
            "検索...".to_string()
        } else {
            self.snippet_palette.query().to_string()
        };
        let active_group = self.snippet_palette.active_group().map(str::to_owned);
        let queue_status_label = self.snippet_palette.queue_status_label();
        let groups = self.snippet_palette.groups().to_vec();
        let selected_store_index = self
            .snippet_palette
            .filtered_entries()
            .get(
                self.snippet_palette
                    .selected_filtered_index()
                    .unwrap_or(usize::MAX),
            )
            .map(|(store_index, _)| *store_index);
        let filtered_items: Vec<(usize, String, String, String, bool)> = self
            .snippet_palette
            .filtered_entries()
            .into_iter()
            .map(|(store_index, snippet)| {
                (
                    store_index,
                    snippet.title.clone(),
                    snippet.content.clone(),
                    snippet.group.clone(),
                    snippet.is_favorite,
                )
            })
            .collect();

        let panel = div()
                .id("snippet-panel")
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.snippet_palette.hide();
                    this.focus_active_terminal(window, cx);
                    cx.notify();
                }))
                .absolute()
                .top(px(top))
                .left(px(left))
                .w(px(panel_w))
                .h(px(panel_h))
                .rounded(px(16.0))
                .border_1()
                .border_color(rgba(0xD9DADDFF))
                .bg(rgb(0xFFFFFF))
                .shadow_lg()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .h(px(40.0))
                        .px(px(14.0))
                        .border_b_1()
                        .border_color(rgba(0xE5E7EBFF))
                        .bg(rgb(0xF8F8F8))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(RED)))
                                .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(YELLOW)))
                                .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(GREEN))),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(12.0))
                                .text_color(rgba(0x6B7280FF))
                                .child("Clibor"),
                        )
                        .child(div().w(px(46.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .w_full()
                        .flex()
                        .overflow_hidden()
                        .child(
                            div()
                                .w(px(152.0))
                                .h_full()
                                .border_r_1()
                                .border_color(rgba(0xE5E7EBFF))
                                .bg(rgb(0xF7F7F8))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .w_full()
                                        .h_full()
                                        .flex_1()
                                        .p(px(8.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .px(px(10.0))
                                                .py(px(7.0))
                                                .rounded(px(7.0))
                                                .cursor_pointer()
                                                .font_family(UI_FONT)
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .bg(if active_group.is_none() {
                                                    rgba(0x2F80EDFF)
                                                } else {
                                                    rgba(0x00000000)
                                                })
                                                .text_color(if active_group.is_none() {
                                                    rgba(0xFFFFFFFF)
                                                } else {
                                                    rgba(0x374151FF)
                                                })
                                                .hover(|style| style.bg(rgba(0xE5E7EBFF)))
                                                .child("すべて")
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                        this.snippet_palette.set_active_group(None);
                                                        cx.notify();
                                                    }),
                                                ),
                                        )
                                        .children(groups.into_iter().map(|group| {
                                            let is_active = active_group
                                                .as_deref()
                                                .map(|active| active.eq_ignore_ascii_case(&group))
                                                .unwrap_or(false);
                                            let group_name = group.clone();
                                            div()
                                                .id(ElementId::Name(
                                                    format!("snippet-group-{group_name}").into(),
                                                ))
                                                .px(px(10.0))
                                                .py(px(7.0))
                                                .rounded(px(7.0))
                                                .cursor_pointer()
                                                .font_family(UI_FONT)
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .bg(if is_active {
                                                    rgba(0x2F80EDFF)
                                                } else {
                                                    rgba(0x00000000)
                                                })
                                                .text_color(if is_active {
                                                    rgba(0xFFFFFFFF)
                                                } else {
                                                    rgba(0x374151FF)
                                                })
                                                .hover(|style| style.bg(rgba(0xE5E7EBFF)))
                                                .child(group.clone())
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(
                                                        move |this,
                                                              _: &MouseDownEvent,
                                                              _window,
                                                              cx| {
                                                            this.snippet_palette
                                                                .set_active_group(Some(group_name.clone()));
                                                            cx.notify();
                                                        },
                                                    ),
                                                )
                                        })),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .p(px(10.0))
                                        .border_t_1()
                                        .border_color(rgba(0xE5E7EBFF))
                                        .child(
                                            div()
                                                .w_full()
                                                .px(px(6.0))
                                                .py(px(8.0))
                                                .rounded(px(7.0))
                                                .cursor_pointer()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .font_family(UI_FONT)
                                                .text_size(px(12.0))
                                                .text_color(rgba(0x4B5563FF))
                                                .hover(|style| style.bg(rgba(0xE5E7EBFF)))
                                                .child(
                                                    svg()
                                                        .path("ui/settings.svg")
                                                        .size(px(12.0))
                                                        .text_color(rgba(0x6B7280FF)),
                                                )
                                                .child("設定")
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                        this.show_settings = true;
                                                        this.show_shell_menu = false;
                                                        this.show_snippet_context_menu = false;
                                                        this.snippet_palette.hide();
                                                        cx.notify();
                                                    }),
                                                ),
                                        ),
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
                                        .w_full()
                                        .p(px(10.0))
                                        .border_b_1()
                                        .border_color(rgba(0xE5E7EBFF))
                                        .child(
                                            div()
                                                .w_full()
                                                .h(px(34.0))
                                                .rounded(px(8.0))
                                                .border_1()
                                                .border_color(rgba(0xD1D5DBFF))
                                                .bg(rgb(0xF9FAFB))
                                                .px(px(10.0))
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .child(
                                                    svg()
                                                        .path("ui/search.svg")
                                                        .size(px(14.0))
                                                        .text_color(rgba(0x9CA3AFFF)),
                                                )
                                                .child(
                                                    div()
                                                        .font_family(UI_FONT)
                                                        .text_size(px(12.0))
                                                        .text_color(if query_is_empty {
                                                            rgba(0x9CA3AFFF)
                                                        } else {
                                                            rgba(0x374151FF)
                                                        })
                                                        .child(display_text),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .w_full()
                                        .overflow_hidden()
                                        .px(px(10.0))
                                        .py(px(12.0))
                                .children(if filtered_items.is_empty() {
                                    vec![
                                        div()
                                            .w_full()
                                            .h_full()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .font_family(UI_FONT)
                                            .text_size(px(13.0))
                                            .text_color(rgba(0xC4C4C7FF))
                                            .child("定型文がありません")
                                            .into_any_element(),
                                    ]
                                } else {
                                    filtered_items
                                        .iter()
                                        .map(|(store_index, title, content, group_name, is_favorite)| {
                                            let index = *store_index;
                                            let is_selected = selected_store_index == Some(index);
                                            let preview = content.replace('\n', " ");
                                            let summary = if preview.is_empty() {
                                                group_name.clone()
                                            } else {
                                                preview
                                            };
                                            div()
                                                .id(ElementId::Name(
                                                    format!("snippet-item-{store_index}").into(),
                                                ))
                                                .mb(px(8.0))
                                                .p(px(12.0))
                                                .rounded(px(10.0))
                                                .border_1()
                                                .border_color(if is_selected {
                                                    rgba(0xBBD1FFFF)
                                                } else {
                                                    rgba(0xE5E7EBFF)
                                                })
                                                .bg(if is_selected {
                                                    rgba(0xEEF5FFFF)
                                                } else {
                                                    rgba(0xFFFFFFFF)
                                                })
                                                .py(px(12.0))
                                                .hover(|style| style.border_color(rgba(0xD1D5DBFF)))
                                                .child(
                                                    div()
                                                        .w_full()
                                                        .flex()
                                                        .items_start()
                                                        .justify_between()
                                                        .gap(px(12.0))
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .flex()
                                                                .flex_col()
                                                                .gap(px(4.0))
                                                                .cursor_pointer()
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    cx.listener(
                                                                        move |this,
                                                                              _: &MouseDownEvent,
                                                                              _window,
                                                                              cx| {
                                                                            this.snippet_palette
                                                                                .select_store_index(index);
                                                                            cx.notify();
                                                                        },
                                                                    ),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .font_family(UI_FONT)
                                                                        .text_size(px(12.0))
                                                                        .font_weight(FontWeight::MEDIUM)
                                                                        .text_color(rgba(0x374151FF))
                                                                        .child(title.clone()),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .font_family(UI_FONT)
                                                                        .text_size(px(11.0))
                                                                        .text_color(rgba(0x9CA3AFFF))
                                                                        .child(summary),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .font_family(UI_FONT)
                                                                        .text_size(px(10.0))
                                                                        .text_color(rgba(0xC4C4C7FF))
                                                                        .child(group_name.clone()),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap(px(6.0))
                                                                .child(
                                                                    snippet_list_icon_button(
                                                                        "ui/edit.svg",
                                                                        rgba(0x9CA3AFFF),
                                                                        rgba(0xF3F4F6FF),
                                                                        cx.listener(
                                                                            move |this,
                                                                                  _: &MouseDownEvent,
                                                                                  _window,
                                                                                  cx| {
                                                                                this.open_snippet_editor(
                                                                                    SnippetEditorMode::Edit(index),
                                                                                    None,
                                                                                );
                                                                                cx.stop_propagation();
                                                                                cx.notify();
                                                                            },
                                                                        ),
                                                                    ),
                                                                )
                                                                .child(
                                                                    snippet_list_icon_button(
                                                                        if *is_favorite {
                                                                            "ui/star-filled.svg"
                                                                        } else {
                                                                            "ui/star.svg"
                                                                        },
                                                                        if *is_favorite {
                                                                            rgba(0xF59E0BFF)
                                                                        } else {
                                                                            rgba(0x9CA3AFFF)
                                                                        },
                                                                        rgba(0xFFFBEBFF),
                                                                        cx.listener(
                                                                            move |this,
                                                                                  _: &MouseDownEvent,
                                                                                  _window,
                                                                                  cx| {
                                                                                let _ = this
                                                                                    .snippet_palette
                                                                                    .toggle_favorite(index);
                                                                                cx.stop_propagation();
                                                                                cx.notify();
                                                                            },
                                                                        ),
                                                                    ),
                                                                )
                                                                .child(
                                                                    snippet_list_icon_button(
                                                                        "ui/trash.svg",
                                                                        rgba(0x9CA3AFFF),
                                                                        rgba(0xFEF2F2FF),
                                                                        cx.listener(
                                                                            move |this,
                                                                                  _: &MouseDownEvent,
                                                                                  _window,
                                                                                  cx| {
                                                                                let _ = this
                                                                                    .snippet_palette
                                                                                    .delete_snippet(index);
                                                                                cx.stop_propagation();
                                                                                cx.notify();
                                                                            },
                                                                        ),
                                                                    ),
                                                                ),
                                                        ),
                                                )
                                                .into_any_element()
                                        })
                                        .collect()
                                }),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(38.0))
                        .px(px(10.0))
                        .border_t_1()
                        .border_color(rgba(0xE5E7EBFF))
                        .bg(rgb(0xF9FAFB))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(11.0))
                                        .text_color(rgba(0x9CA3AFFF))
                                        .child(format!("{} 件の定型文", filtered_items.len())),
                                )
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(11.0))
                                        .text_color(rgba(0xC4C4C7FF))
                                        .child(queue_status_label),
                                )
                        )
                        .child(
                            div()
                                .h(px(24.0))
                                .px(px(10.0))
                                .rounded(px(8.0))
                                .border_1()
                                .border_color(rgba(0xE5E7EBFF))
                                .bg(rgb(0xFFFFFF))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .hover(|style| style.bg(rgba(0xF3F4F6FF)))
                                .child(
                                    svg()
                                        .path("ui/plus.svg")
                                        .size(px(12.0))
                                        .text_color(rgba(0x6B7280FF)),
                                )
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(11.0))
                                        .text_color(rgba(0x374151FF))
                                        .child("追加"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                        let group_hint = this
                                            .snippet_palette
                                            .active_group()
                                            .and_then(|group| this.snippet_palette.group_index_by_name(group));
                                        this.open_snippet_editor(SnippetEditorMode::Add, group_hint);
                                        cx.notify();
                                    }),
                                ),
                        ),
                ));
        Some(panel.into_any_element())
    }

    fn render_snippet_context_menu(
        &mut self,
        viewport_w: f32,
        _viewport_h: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_snippet_context_menu {
            return None;
        }

        let menu_w = 312.0;
        let top = 42.0;
        let left = (viewport_w - menu_w - 10.0).max(12.0);
        let fifo_active = self.snippet_palette.queue_mode() == SnippetQueueMode::Fifo;
        let lifo_active = self.snippet_palette.queue_mode() == SnippetQueueMode::Lifo;

        Some(
            div()
                .id("snippet-context-menu-overlay")
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .w_full()
                .h_full()
                .child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .w_full()
                        .h_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                this.show_snippet_context_menu = false;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                this.show_snippet_context_menu = false;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .id("snippet-context-menu")
                        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.show_snippet_context_menu = false;
                            cx.notify();
                        }))
                        .absolute()
                        .top(px(top))
                        .left(px(left))
                        .w(px(menu_w))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(rgba(0x313244FF))
                        .bg(rgba(0x11111BFF))
                        .shadow_lg()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .child(snippet_menu_item(
                            "定型文グループの編集(G)",
                            false,
                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                this.show_snippet_context_menu = false;
                                this.open_snippet_group_editor();
                                cx.notify();
                            }),
                        ))
                        .child(snippet_menu_item(
                            "定型文の編集(T)",
                            false,
                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                this.show_snippet_context_menu = false;
                                this.open_snippet_list_editor();
                                cx.notify();
                            }),
                        ))
                        .child(snippet_menu_item(
                            "定型文CSV出力/取込(V)",
                            false,
                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                this.show_snippet_context_menu = false;
                                this.open_snippet_csv_dialog();
                                cx.notify();
                            }),
                        ))
                        .child(div().w_full().h(px(1.0)).my(px(4.0)).bg(rgba(0x313244FF)))
                        .child(snippet_menu_item(
                            "FIFOモード(O)",
                            fifo_active,
                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                this.show_snippet_context_menu = false;
                                this.snippet_palette.set_fifo_mode();
                                cx.notify();
                            }),
                        ))
                        .child(snippet_menu_item(
                            "LIFOモード(L)",
                            lifo_active,
                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                this.show_snippet_context_menu = false;
                                this.snippet_palette.set_lifo_mode();
                                cx.notify();
                            }),
                        )),
                )
                .into_any_element(),
        )
    }

    fn render_snippet_group_editor(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let editor = self.snippet_group_editor.as_ref()?.clone();
        let groups = self.snippet_palette.groups().to_vec();
        let selected_index = editor.selected_index.min(groups.len().saturating_sub(1));
        let panel_w = 880.0;
        let panel_h = 520.0;
        let top = ((viewport_h - panel_h) * 0.5).max(24.0);
        let left = ((viewport_w - panel_w) * 0.5).max(24.0);
        let editing = editor.edit_mode != SnippetGroupEditMode::None;

        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, group_name) in groups.iter().cloned().enumerate() {
            let row_index = index;
            let is_selected = index == selected_index;
            rows.push(
                div()
                    .id(ElementId::Name(
                        format!("snippet-group-row-{row_index}").into(),
                    ))
                    .w_full()
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(rgba(0xffffff08))
                    .bg(if is_selected {
                        rgba(0xffffff10)
                    } else {
                        rgba(0x00000000)
                    })
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(0xffffff12)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            if let Some(editor) = this.snippet_group_editor.as_mut() {
                                editor.selected_index = row_index;
                                cx.notify();
                            }
                        }),
                    )
                    .child(
                        div()
                            .font_family("JetBrains Mono")
                            .text_size(px(12.0))
                            .text_color(rgb(if is_selected { TEXT } else { TEXT_SOFT }))
                            .child(group_name),
                    )
                    .into_any_element(),
            );
        }

        Some(
            div()
                .id("snippet-group-editor-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(rgba(BACKDROP)),
                )
                .child(
                    div()
                        .id("snippet-group-editor-panel")
                        .absolute()
                        .top(px(top))
                        .left(px(left))
                        .w(px(panel_w))
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(rgba(0xffffff10))
                        .bg(rgb(WINDOW_BG))
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.close_snippet_group_editor();
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w_full()
                                .px(px(16.0))
                                .py(px(12.0))
                                .border_b_1()
                                .border_color(rgba(0xffffff10))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(14.0))
                                        .text_color(rgb(TEXT))
                                        .child("定型文グループ"),
                                )
                                .child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(11.0))
                                        .text_color(rgb(SUBTEXT1))
                                        .child("Enter:確定 Esc:閉じる"),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .gap(px(12.0))
                                .p(px(12.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h(px(400.0))
                                        .rounded(px(10.0))
                                        .border_1()
                                        .border_color(rgba(0xffffff10))
                                        .bg(rgba(0xffffff06))
                                        .overflow_hidden()
                                        .children(rows),
                                )
                                .child(
                                    div()
                                        .w(px(220.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(interactive_action_button(
                                            "名前変更",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let selected = this
                                                    .snippet_group_editor
                                                    .as_ref()
                                                    .map(|editor| editor.selected_index)
                                                    .unwrap_or(0);
                                                let current_name = this
                                                    .snippet_palette
                                                    .group_name_at(selected)
                                                    .unwrap_or("")
                                                    .to_string();
                                                if let Some(editor) =
                                                    this.snippet_group_editor.as_mut()
                                                {
                                                    editor.selected_index = selected;
                                                    editor.edit_mode =
                                                        SnippetGroupEditMode::Rename(selected);
                                                    editor.draft_name = current_name;
                                                    cx.notify();
                                                }
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            "上へ",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let selected = this
                                                    .snippet_group_editor
                                                    .as_ref()
                                                    .map(|editor| editor.selected_index)
                                                    .unwrap_or(0);
                                                if let Some(next_index) =
                                                    this.snippet_palette.move_group_up(selected)
                                                {
                                                    if let Some(editor) =
                                                        this.snippet_group_editor.as_mut()
                                                    {
                                                        editor.selected_index = next_index;
                                                    }
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            "下へ",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let selected = this
                                                    .snippet_group_editor
                                                    .as_ref()
                                                    .map(|editor| editor.selected_index)
                                                    .unwrap_or(0);
                                                if let Some(next_index) =
                                                    this.snippet_palette.move_group_down(selected)
                                                {
                                                    if let Some(editor) =
                                                        this.snippet_group_editor.as_mut()
                                                    {
                                                        editor.selected_index = next_index;
                                                    }
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(div().h(px(8.0)))
                                        .child(interactive_action_button(
                                            "新規追加",
                                            true,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                if let Some(editor) =
                                                    this.snippet_group_editor.as_mut()
                                                {
                                                    editor.edit_mode = SnippetGroupEditMode::New;
                                                    editor.draft_name.clear();
                                                    cx.notify();
                                                }
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            "削除",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let selected = this
                                                    .snippet_group_editor
                                                    .as_ref()
                                                    .map(|editor| editor.selected_index)
                                                    .unwrap_or(0);
                                                if this.snippet_palette.delete_group(selected) {
                                                    if let Some(editor) =
                                                        this.snippet_group_editor.as_mut()
                                                    {
                                                        editor.selected_index =
                                                            selected.saturating_sub(1);
                                                        editor.edit_mode =
                                                            SnippetGroupEditMode::None;
                                                        editor.draft_name.clear();
                                                    }
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            if editing { "確定" } else { "閉じる" },
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                if this
                                                    .snippet_group_editor
                                                    .as_ref()
                                                    .map(|editor| {
                                                        editor.edit_mode
                                                            != SnippetGroupEditMode::None
                                                    })
                                                    .unwrap_or(false)
                                                {
                                                    this.apply_group_editor_edit();
                                                } else {
                                                    this.close_snippet_group_editor();
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(div().h(px(8.0)))
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(11.0))
                                                .text_color(rgb(SUBTEXT1))
                                                .child(format!(
                                                    "最大 {} グループ",
                                                    crate::snippets::SnippetStore::MAX_GROUPS
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .p(px(12.0))
                                .border_t_1()
                                .border_color(rgba(0xffffff10))
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(12.0))
                                        .text_color(rgb(TEXT_SOFT))
                                        .child(if editing {
                                            "グループ名"
                                        } else {
                                            "名前変更または新規追加を押すとここで編集できます"
                                        }),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(38.0))
                                        .rounded(px(8.0))
                                        .border_1()
                                        .border_color(rgb(if editing { ACCENT } else { SURFACE1 }))
                                        .bg(rgba(0xffffff08))
                                        .px(px(12.0))
                                        .flex()
                                        .items_center()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(12.0))
                                        .text_color(rgb(TEXT))
                                        .child(if editing {
                                            editor.draft_name
                                        } else {
                                            String::new()
                                        }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_snippet_list_editor(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.snippet_list_editor.as_ref()?.clone();
        let groups = self.snippet_palette.groups().to_vec();
        let selected_group_index = state
            .selected_group_index
            .min(groups.len().saturating_sub(1));
        let selected_group_name = groups.get(selected_group_index)?.clone();
        let items = self
            .snippet_palette
            .snippets_for_group(Some(&selected_group_name));
        let panel_w = 1020.0;
        let panel_h = 620.0;
        let top = ((viewport_h - panel_h) * 0.5).max(24.0);
        let left = ((viewport_w - panel_w) * 0.5).max(24.0);

        let mut rows: Vec<AnyElement> = Vec::new();
        for (row_index, (store_index, snippet)) in items.iter().enumerate() {
            let store_index = *store_index;
            let is_selected = state.selected_item_store_index == Some(store_index);
            let preview = snippet.content.replace('\n', " ");
            rows.push(
                div()
                    .id(ElementId::Name(
                        format!("snippet-list-row-{store_index}").into(),
                    ))
                    .w_full()
                    .flex()
                    .border_b_1()
                    .border_color(rgba(0xffffff08))
                    .bg(if is_selected {
                        rgba(0xffffff10)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|style| style.bg(rgba(0xffffff12)))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            if let Some(state) = this.snippet_list_editor.as_mut() {
                                state.selected_item_store_index = Some(store_index);
                                cx.notify();
                            }
                        }),
                    )
                    .child(
                        div()
                            .w(px(260.0))
                            .px(px(10.0))
                            .py(px(8.0))
                            .font_family("JetBrains Mono")
                            .text_size(px(12.0))
                            .text_color(rgb(TEXT))
                            .child(snippet.title.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .px(px(10.0))
                            .py(px(8.0))
                            .border_l_1()
                            .border_color(rgba(0xffffff08))
                            .font_family("JetBrains Mono")
                            .text_size(px(12.0))
                            .text_color(rgb(TEXT_SOFT))
                            .child(preview),
                    )
                    .child(
                        div()
                            .w(px(84.0))
                            .px(px(10.0))
                            .py(px(8.0))
                            .border_l_1()
                            .border_color(rgba(0xffffff08))
                            .font_family("JetBrains Mono")
                            .text_size(px(11.0))
                            .text_color(rgb(SUBTEXT1))
                            .text_right()
                            .child(format!("{}", row_index + 1)),
                    )
                    .into_any_element(),
            );
        }

        Some(
            div()
                .id("snippet-list-editor-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(rgba(BACKDROP)),
                )
                .child(
                    div()
                        .id("snippet-list-editor-panel")
                        .absolute()
                        .top(px(top))
                        .left(px(left))
                        .w(px(panel_w))
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(rgba(0xffffff10))
                        .bg(rgb(WINDOW_BG))
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.close_snippet_list_editor();
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w_full()
                                .px(px(16.0))
                                .py(px(12.0))
                                .border_b_1()
                                .border_color(rgba(0xffffff10))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(14.0))
                                        .text_color(rgb(TEXT))
                                        .child("定型文"),
                                )
                                .child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(11.0))
                                        .text_color(rgb(SUBTEXT1))
                                        .child("Ctrl+N:新規 Enter:編集 Del:削除"),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .px(px(16.0))
                                .py(px(12.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(10.0))
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(12.0))
                                                .text_color(rgb(TEXT_SOFT))
                                                .child("定型文グループ"),
                                        )
                                        .child(interactive_select_box(
                                            selected_group_name.clone(),
                                            260.0,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let total =
                                                    this.snippet_palette.groups().len().max(1);
                                                let next_group_index = this
                                                    .snippet_list_editor
                                                    .as_ref()
                                                    .map(|state| {
                                                        (state.selected_group_index + 1) % total
                                                    })
                                                    .unwrap_or(0);
                                                let next_selection = this
                                                    .snippet_palette
                                                    .group_name_at(next_group_index)
                                                    .and_then(|group| {
                                                        this.snippet_palette
                                                            .snippets_for_group(Some(group))
                                                            .first()
                                                            .map(|(store_index, _)| *store_index)
                                                    });
                                                if let Some(state) =
                                                    this.snippet_list_editor.as_mut()
                                                {
                                                    state.selected_group_index = next_group_index;
                                                    state.selected_item_store_index =
                                                        next_selection;
                                                    cx.notify();
                                                }
                                            }),
                                        )),
                                )
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(11.0))
                                        .text_color(rgb(SUBTEXT1))
                                        .child(format!(
                                            "{} / {}",
                                            items.len(),
                                            crate::snippets::SnippetStore::MAX_SNIPPETS_PER_GROUP
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .gap(px(12.0))
                                .px(px(16.0))
                                .pb(px(16.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h(px(460.0))
                                        .rounded(px(10.0))
                                        .border_1()
                                        .border_color(rgba(0xffffff10))
                                        .bg(rgba(0xffffff06))
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .border_b_1()
                                                .border_color(rgba(0xffffff10))
                                                .bg(rgba(0xffffff08))
                                                .child(
                                                    div()
                                                        .w(px(260.0))
                                                        .px(px(10.0))
                                                        .py(px(8.0))
                                                        .font_family(UI_FONT)
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(SUBTEXT1))
                                                        .child("定型文"),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .px(px(10.0))
                                                        .py(px(8.0))
                                                        .border_l_1()
                                                        .border_color(rgba(0xffffff10))
                                                        .font_family(UI_FONT)
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(SUBTEXT1))
                                                        .child("内容"),
                                                )
                                                .child(
                                                    div()
                                                        .w(px(84.0))
                                                        .px(px(10.0))
                                                        .py(px(8.0))
                                                        .border_l_1()
                                                        .border_color(rgba(0xffffff10))
                                                        .font_family(UI_FONT)
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(SUBTEXT1))
                                                        .child("No."),
                                                ),
                                        )
                                        .children(rows),
                                )
                                .child(
                                    div()
                                        .w(px(220.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(interactive_action_button(
                                            "上へ",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let selected =
                                                    this.snippet_list_editor.as_ref().and_then(
                                                        |state| state.selected_item_store_index,
                                                    );
                                                if let Some(selected) = selected {
                                                    let _ = this
                                                        .snippet_palette
                                                        .move_snippet_up(selected);
                                                    this.sync_list_editor_selection();
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            "下へ",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let selected =
                                                    this.snippet_list_editor.as_ref().and_then(
                                                        |state| state.selected_item_store_index,
                                                    );
                                                if let Some(selected) = selected {
                                                    let _ = this
                                                        .snippet_palette
                                                        .move_snippet_down(selected);
                                                    this.sync_list_editor_selection();
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(div().h(px(8.0)))
                                        .child(interactive_action_button(
                                            "新規登録",
                                            true,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let group_hint = this
                                                    .snippet_list_editor
                                                    .as_ref()
                                                    .map(|state| state.selected_group_index);
                                                this.open_snippet_editor(
                                                    SnippetEditorMode::Add,
                                                    group_hint,
                                                );
                                                cx.notify();
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            "編集",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let selected =
                                                    this.snippet_list_editor.as_ref().and_then(
                                                        |state| state.selected_item_store_index,
                                                    );
                                                if let Some(selected) = selected {
                                                    this.open_snippet_editor(
                                                        SnippetEditorMode::Edit(selected),
                                                        None,
                                                    );
                                                    cx.notify();
                                                }
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            "削除",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let selected =
                                                    this.snippet_list_editor.as_ref().and_then(
                                                        |state| state.selected_item_store_index,
                                                    );
                                                if let Some(selected) = selected {
                                                    if this.snippet_palette.delete_snippet(selected)
                                                    {
                                                        this.sync_list_editor_selection();
                                                    }
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            "移動",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                let groups = this.snippet_palette.groups().to_vec();
                                                let Some(state) = this.snippet_list_editor.as_ref()
                                                else {
                                                    return;
                                                };
                                                let Some(selected) =
                                                    state.selected_item_store_index
                                                else {
                                                    return;
                                                };
                                                if groups.len() < 2 {
                                                    return;
                                                }
                                                if let Some(next_group) = groups
                                                    .get(
                                                        (state.selected_group_index + 1)
                                                            % groups.len(),
                                                    )
                                                    .cloned()
                                                {
                                                    if this.snippet_palette.move_snippet_to_group(
                                                        selected,
                                                        &next_group,
                                                    ) {
                                                        this.sync_list_editor_selection();
                                                    }
                                                    cx.notify();
                                                }
                                            }),
                                        ))
                                        .child(div().h(px(8.0)))
                                        .child(interactive_action_button(
                                            "閉じる",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                this.close_snippet_list_editor();
                                                cx.notify();
                                            }),
                                        )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_snippet_editor(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let editor = self.snippet_editor.as_ref()?.clone();
        let groups = self.snippet_palette.groups().to_vec();
        let panel_w = 720.0;
        let top = ((viewport_h - 420.0) * 0.5).max(48.0);
        let left = ((viewport_w - panel_w) * 0.5).max(48.0);
        let group_label = groups
            .get(editor.group_index.min(groups.len().saturating_sub(1)))
            .cloned()
            .unwrap_or_else(|| "General".to_string());
        let mode_label = match editor.mode {
            SnippetEditorMode::Add => "定型文の新規登録",
            SnippetEditorMode::Edit(_) => "定型文の編集",
        };
        let title_active = editor.active_field == SnippetEditorField::Title;
        let content_active = editor.active_field == SnippetEditorField::Content;

        Some(
            div()
                .id("snippet-editor-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(rgba(BACKDROP)),
                )
                .child(
                    div()
                        .id("snippet-editor-panel")
                        .absolute()
                        .top(px(top))
                        .left(px(left))
                        .w(px(panel_w))
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(rgba(0xffffff10))
                        .bg(rgb(WINDOW_BG))
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.close_snippet_editor();
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w_full()
                                .px(px(16.0))
                                .py(px(12.0))
                                .border_b_1()
                                .border_color(rgba(0xffffff10))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(14.0))
                                        .text_color(rgb(TEXT))
                                        .child(mode_label),
                                )
                                .child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(11.0))
                                        .text_color(rgb(SUBTEXT1))
                                        .child("Tab:項目切替 Ctrl+Enter:保存"),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .p(px(16.0))
                                .flex()
                                .flex_col()
                                .gap(px(12.0))
                                .child(settings_row(
                                    "定型文グループ",
                                    interactive_select_box(
                                        group_label,
                                        220.0,
                                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                            let total = this.snippet_palette.groups().len().max(1);
                                            if let Some(editor) = this.snippet_editor.as_mut() {
                                                editor.group_index =
                                                    (editor.group_index + 1) % total;
                                                cx.notify();
                                            }
                                        }),
                                    ),
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(12.0))
                                                .text_color(rgb(TEXT_SOFT))
                                                .child("タイトル"),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .h(px(40.0))
                                                .rounded(px(8.0))
                                                .border_1()
                                                .border_color(rgb(if title_active {
                                                    ACCENT
                                                } else {
                                                    SURFACE1
                                                }))
                                                .bg(rgba(0xffffff08))
                                                .px(px(12.0))
                                                .flex()
                                                .items_center()
                                                .font_family("JetBrains Mono")
                                                .text_size(px(12.0))
                                                .text_color(rgb(TEXT))
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(
                                                        |this, _: &MouseDownEvent, _window, cx| {
                                                            if let Some(editor) =
                                                                this.snippet_editor.as_mut()
                                                            {
                                                                editor.active_field =
                                                                    SnippetEditorField::Title;
                                                                cx.notify();
                                                            }
                                                        },
                                                    ),
                                                )
                                                .child(editor.title),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(12.0))
                                                .text_color(rgb(TEXT_SOFT))
                                                .child("内容"),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .min_h(px(180.0))
                                                .rounded(px(8.0))
                                                .border_1()
                                                .border_color(rgb(if content_active {
                                                    ACCENT
                                                } else {
                                                    SURFACE1
                                                }))
                                                .bg(rgba(0xffffff08))
                                                .p(px(12.0))
                                                .font_family("JetBrains Mono")
                                                .text_size(px(12.0))
                                                .text_color(rgb(TEXT))
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(
                                                        |this, _: &MouseDownEvent, _window, cx| {
                                                            if let Some(editor) =
                                                                this.snippet_editor.as_mut()
                                                            {
                                                                editor.active_field =
                                                                    SnippetEditorField::Content;
                                                                cx.notify();
                                                            }
                                                        },
                                                    ),
                                                )
                                                .child(editor.content),
                                        ),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .justify_end()
                                        .gap(px(8.0))
                                        .child(interactive_action_button(
                                            "キャンセル",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                this.close_snippet_editor();
                                                cx.notify();
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            "保存",
                                            true,
                                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                this.save_snippet_editor();
                                                cx.notify();
                                            }),
                                        )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_snippet_csv_dialog(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.snippet_csv_dialog.as_ref()?.clone();
        let panel_w = 760.0;
        let top = ((viewport_h - 360.0) * 0.5).max(48.0);
        let left = ((viewport_w - panel_w) * 0.5).max(48.0);

        Some(
            div()
                .id("snippet-csv-dialog-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(div().absolute().top_0().left_0().size_full().bg(rgba(BACKDROP)))
                .child(
                    div()
                        .id("snippet-csv-dialog-panel")
                        .absolute()
                        .top(px(top))
                        .left(px(left))
                        .w(px(panel_w))
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(rgba(0xffffff10))
                        .bg(rgb(WINDOW_BG))
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down_out(cx.listener(
                            |this, _: &MouseDownEvent, _window, cx| {
                                this.close_snippet_csv_dialog();
                                cx.notify();
                            },
                        ))
                        .child(
                            div()
                                .w_full()
                                .px(px(16.0))
                                .py(px(12.0))
                                .border_b_1()
                                .border_color(rgba(0xffffff10))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(14.0))
                                        .text_color(rgb(TEXT))
                                        .child("定型文CSV出力/取込"),
                                )
                                .child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(11.0))
                                        .text_color(rgb(SUBTEXT1))
                                        .child("smux 互換: UTF-8 / Shift_JIS"),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .p(px(16.0))
                                .flex()
                                .flex_col()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .rounded(px(10.0))
                                        .border_1()
                                        .border_color(rgba(0xffffff10))
                                        .bg(rgba(0xffffff06))
                                        .p(px(14.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(10.0))
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(13.0))
                                                .text_color(rgb(TEXT))
                                                .child("CSV出力"),
                                        )
                                        .child(settings_row(
                                            "文字コード",
                                            interactive_select_box(
                                                csv_encoding_label(state.export_encoding).to_string(),
                                                180.0,
                                                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                    if let Some(state) = this.snippet_csv_dialog.as_mut() {
                                                        state.export_encoding =
                                                            next_csv_encoding(state.export_encoding);
                                                        cx.notify();
                                                    }
                                                }),
                                            ),
                                        ))
                                        .child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .justify_end()
                                                .child(interactive_action_button(
                                                    "出力",
                                                    true,
                                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                        let encoding = this
                                                            .snippet_csv_dialog
                                                            .as_ref()
                                                            .map(|state| state.export_encoding)
                                                            .unwrap_or(CsvEncoding::ShiftJis);
                                                        let path = rfd::FileDialog::new()
                                                            .add_filter("CSV", &["csv"])
                                                            .set_file_name(&default_snippet_csv_file_name())
                                                            .save_file();
                                                        if let Some(path) = path {
                                                            if let Err(err) = this
                                                                .snippet_palette
                                                                .export_csv_with_encoding(path.as_path(), encoding)
                                                            {
                                                                log::warn!("Failed to export snippets csv: {}", err);
                                                            }
                                                        }
                                                        cx.notify();
                                                    }),
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .rounded(px(10.0))
                                        .border_1()
                                        .border_color(rgba(0xffffff10))
                                        .bg(rgba(0xffffff06))
                                        .p(px(14.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(10.0))
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(13.0))
                                                .text_color(rgb(TEXT))
                                                .child("CSV取込"),
                                        )
                                        .child(settings_row(
                                            "文字コード",
                                            interactive_select_box(
                                                csv_encoding_label(state.import_encoding).to_string(),
                                                180.0,
                                                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                    if let Some(state) = this.snippet_csv_dialog.as_mut() {
                                                        state.import_encoding =
                                                            next_csv_encoding(state.import_encoding);
                                                        cx.notify();
                                                    }
                                                }),
                                            ),
                                        ))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                        if let Some(state) = this.snippet_csv_dialog.as_mut() {
                                                            state.clear_before_import = !state.clear_before_import;
                                                            cx.notify();
                                                        }
                                                    }),
                                                )
                                                .child(
                                                    div()
                                                        .w(px(16.0))
                                                        .h(px(16.0))
                                                        .rounded(px(4.0))
                                                        .border_1()
                                                        .border_color(rgb(SURFACE1))
                                                        .bg(if state.clear_before_import {
                                                            rgb(ACCENT)
                                                        } else {
                                                            rgba(0xffffff06)
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .font_family(UI_FONT)
                                                        .text_size(px(12.0))
                                                        .text_color(rgb(TEXT_SOFT))
                                                        .child("既存の定型文を全てクリアしてから取り込む"),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(11.0))
                                                .text_color(rgb(SUBTEXT1))
                                                .child("同名グループは維持しつつ、CSV内容で更新します。"),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .justify_end()
                                                .gap(px(8.0))
                                                .child(interactive_action_button(
                                                    "閉じる",
                                                    false,
                                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                        this.close_snippet_csv_dialog();
                                                        cx.notify();
                                                    }),
                                                ))
                                                .child(interactive_action_button(
                                                    "取込",
                                                    true,
                                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                                        let (encoding, clear_before_import) = this
                                                            .snippet_csv_dialog
                                                            .as_ref()
                                                            .map(|state| {
                                                                (state.import_encoding, state.clear_before_import)
                                                            })
                                                            .unwrap_or((CsvEncoding::ShiftJis, false));
                                                        let path = rfd::FileDialog::new()
                                                            .add_filter("CSV", &["csv"])
                                                            .pick_file();
                                                        if let Some(path) = path {
                                                            if let Err(err) = this.snippet_palette.import_csv_with_encoding(
                                                                path.as_path(),
                                                                encoding,
                                                                clear_before_import,
                                                            ) {
                                                                log::warn!("Failed to import snippets csv: {}", err);
                                                            }
                                                        }
                                                        cx.notify();
                                                    }),
                                                )),
                                        ),
                                ),
                        ),
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
                    this.show_snippet_context_menu = false;
                    this.snippet_palette.hide();
                    this.show_shell_menu = true;
                    cx.notify();
                }),
            ))
            .child(
                chrome_button("title-shells", "ui/chevron-down.svg").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.show_settings = false;
                        this.show_snippet_context_menu = false;
                        this.snippet_palette.hide();
                        this.show_shell_menu = true;
                        cx.notify();
                    }),
                ),
            )
            .child(
                chrome_button("title-snippets", "ui/snippets.svg")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.show_shell_menu = false;
                            this.show_settings = false;
                            this.show_close_confirm = false;
                            this.show_snippet_context_menu = false;
                            this.snippet_palette.toggle();
                            if this.snippet_palette.is_visible() {
                                this.focus_handle.focus(window);
                            } else {
                                this.focus_active_terminal(window, cx);
                            }
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.show_shell_menu = false;
                            this.show_settings = false;
                            this.show_close_confirm = false;
                            this.snippet_palette.hide();
                            this.show_snippet_context_menu = !this.show_snippet_context_menu;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                chrome_button("title-settings", "ui/settings.svg").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.show_shell_menu = false;
                        this.show_snippet_context_menu = false;
                        this.snippet_palette.hide();
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

        let snippet_backdrop = if self.snippet_palette.is_visible() {
            Some(
                div()
                    .id("snippet-backdrop")
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
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_split_right))
            .on_action(cx.listener(Self::on_split_down))
            .on_action(cx.listener(Self::on_close_pane))
            .on_action(cx.listener(Self::on_focus_next))
            .on_action(cx.listener(Self::on_focus_prev))
            .on_action(cx.listener(Self::on_toggle_snippet_palette))
            .on_action(cx.listener(Self::on_snippet_queue_paste))
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
            .children(self.render_snippet_context_menu(
                new_state.width,
                new_state.height,
                window,
                cx,
            ))
            .children(snippet_backdrop)
            .children(self.render_snippet_palette(new_state.width, new_state.height, window, cx))
            .children(self.render_snippet_group_editor(
                new_state.width,
                new_state.height,
                window,
                cx,
            ))
            .children(self.render_snippet_list_editor(
                new_state.width,
                new_state.height,
                window,
                cx,
            ))
            .children(self.render_snippet_editor(new_state.width, new_state.height, window, cx))
            .children(self.render_snippet_csv_dialog(new_state.width, new_state.height, window, cx))
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

fn default_snippet_csv_file_name() -> String {
    crate::snippets::SnippetStore::csv_path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("snippets.csv")
        .to_string()
}

fn csv_encoding_label(encoding: CsvEncoding) -> &'static str {
    match encoding {
        CsvEncoding::Utf8 => "UTF-8",
        CsvEncoding::ShiftJis => "Shift_JIS",
    }
}

fn next_csv_encoding(encoding: CsvEncoding) -> CsvEncoding {
    match encoding {
        CsvEncoding::Utf8 => CsvEncoding::ShiftJis,
        CsvEncoding::ShiftJis => CsvEncoding::Utf8,
    }
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

fn snippet_menu_item(
    label: &'static str,
    active: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w_full()
        .px(px(16.0))
        .py(px(12.0))
        .cursor_pointer()
        .font_family(UI_FONT)
        .text_size(px(13.0))
        .text_color(if active {
            rgba(0x89b4faFF)
        } else {
            rgba(0xBAC2DEFF)
        })
        .hover(|style| style.bg(rgba(0x313244AA)))
        .on_mouse_down(MouseButton::Left, listener)
        .child(label)
}

fn snippet_list_icon_button(
    icon_path: &'static str,
    icon_color: Rgba,
    hover_bg: Rgba,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(20.0))
        .h(px(20.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .hover(move |style| style.bg(hover_bg))
        .child(svg().path(icon_path).size(px(12.0)).text_color(icon_color))
        .on_mouse_down(MouseButton::Left, listener)
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
