//! Application state and root view — Figma-aligned macOS terminal chrome

use std::io::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, HTCAPTION, PostMessageW, SC_MOVE, SW_RESTORE, ShowWindowAsync,
    WM_SYSCOMMAND,
};

use crate::ai::{
    AiDirectClient, AiProvider, AiSuggestion, AiSuggestionState, AiSuggestionStatus,
    QUERY_MIN_CHARS, default_model_for_provider, next_ai_provider, resolve_ai_api_key,
    resolve_ai_model, sanitize_ai_provider,
};
use crate::config::{
    AppConfig, DEFAULT_TERMINAL_FONT_FAMILY, DEFAULT_UI_FONT_FAMILY,
    SUPPORTED_TERMINAL_FONT_FAMILIES, WindowState, set_launch_on_login,
};
use crate::shell::{self, ShellType};
use crate::snippets::{CsvEncoding, SnippetPalette, SnippetQueueMode, SnippetSettings};
use crate::split::{FocusDir, SplitContainer, SplitDirection};
use crate::terminal::TerminalSettings;
use crate::terminal::view::{CELL_HEIGHT_ESTIMATE, CELL_WIDTH_ESTIMATE, WINDOW_CHROME_HEIGHT};
use crate::{
    ClosePane, CloseTab, FocusNext, FocusPrev, NewTab, OpenSettings, Quit, SnippetQueuePaste,
    SplitDown, SplitRight, ToggleSnippetPalette,
};

const WINDOW_BG: u32 = 0x1C1C1E;
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
const UI_FONT: &str = DEFAULT_UI_FONT_FAMILY;
const MONO_FONT: &str = DEFAULT_TERMINAL_FONT_FAMILY;
const WINDOW_CHROME_RADIUS: f32 = 14.0;
const AI_SUGGEST_DEBOUNCE_MS: u64 = 280;

static SNIPPET_IME_VK_PROCESSKEY: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
unsafe extern "system" fn snippet_ime_getmessage_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, MSG, PM_REMOVE, TranslateMessage, WM_KEYDOWN,
    };

    if code >= 0 && wparam.0 == PM_REMOVE.0 as usize {
        unsafe {
            let msg = &*(lparam.0 as *const MSG);
            if msg.message == WM_KEYDOWN {
                let vk = (msg.wParam.0 & 0xFFFF) as u16;
                if vk == 0xE5 {
                    let _ = TranslateMessage(msg as *const MSG);
                    SNIPPET_IME_VK_PROCESSKEY.store(true, Ordering::Release);
                }
            }
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(target_os = "windows")]
fn install_snippet_ime_hook() {
    use std::sync::Once;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_GETMESSAGE};

    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let thread_id = GetCurrentThreadId();
        match SetWindowsHookExW(
            WH_GETMESSAGE,
            Some(snippet_ime_getmessage_hook_proc),
            None,
            thread_id,
        ) {
            Ok(_) => log::info!("Snippet IME GetMessage hook installed"),
            Err(e) => log::error!("Failed to install snippet IME hook: {}", e),
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn install_snippet_ime_hook() {}

fn should_defer_snippet_keystroke_to_ime(keystroke: &Keystroke) -> bool {
    let ime_processkey_pending = SNIPPET_IME_VK_PROCESSKEY.swap(false, Ordering::AcqRel);
    if !ime_processkey_pending {
        return false;
    }

    if let Some(text) = direct_text_from_snippet_keystroke(keystroke) {
        // Keep direct IME-resolved non-ASCII characters (e.g. あ, 漢字候補確定) in snippet inputs,
        // but continue deferring ASCII keystrokes during IME processing.
        return text.chars().all(|ch| ch.is_ascii());
    }

    !keystroke
        .key_char
        .as_ref()
        .is_some_and(|key_char| !key_char.is_empty())
}

fn direct_text_from_snippet_keystroke(keystroke: &Keystroke) -> Option<String> {
    if keystroke.modifiers.control || keystroke.modifiers.alt {
        return None;
    }

    if let Some(text) = keystroke.key_char.as_ref().filter(|text| !text.is_empty()) {
        return Some(text.clone());
    }

    let key: &str = keystroke.key.as_ref();
    if key.chars().count() == 1 {
        return Some(key.to_string());
    }
    if key == "space" {
        return Some(" ".to_string());
    }

    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ZoomAction {
    Maximize,
    Restore,
}

fn zoom_action_for_window(is_maximized: bool) -> ZoomAction {
    if is_maximized {
        ZoomAction::Restore
    } else {
        ZoomAction::Maximize
    }
}

#[cfg(target_os = "windows")]
fn start_titlebar_drag(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(win32.hwnd.get() as *mut core::ffi::c_void);

    unsafe {
        let _ = ReleaseCapture();
        let _ = PostMessageW(
            Some(hwnd),
            WM_SYSCOMMAND,
            WPARAM((SC_MOVE as usize) | (HTCAPTION as usize)),
            LPARAM(0),
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn start_titlebar_drag(window: &Window) {
    window.start_window_move();
}

#[cfg(target_os = "windows")]
fn try_restore_window(window: &Window) -> bool {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return false;
    };

    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => {
            let hwnd = HWND(handle.hwnd.get() as *mut core::ffi::c_void);
            let target = unsafe { GetAncestor(hwnd, GA_ROOT) };
            let target = if target.is_invalid() { hwnd } else { target };
            unsafe { ShowWindowAsync(target, SW_RESTORE).ok().is_ok() }
        }
        _ => false,
    }
}

#[cfg(not(target_os = "windows"))]
fn try_restore_window(_window: &Window) -> bool {
    false
}

/// Minimum interval between window state saves.
const WINDOW_STATE_SAVE_INTERVAL_SECS: u64 = 2;

/// Theme preview cards in the settings panel.
const THEME_PREVIEWS: [(&str, u32); 3] = [
    ("Catppuccin Mocha", 0x1E1E2E),
    ("Catppuccin Latte", 0xEFF1F5),
    ("Tokyo Night", 0x1A1B26),
];

const APPEARANCE_FONT_FAMILIES: [&str; 3] = SUPPORTED_TERMINAL_FONT_FAMILIES;

fn cycle_string_option(current: &str, options: &[&str], delta: i32) -> String {
    if options.is_empty() {
        return current.to_string();
    }

    let current_index = options
        .iter()
        .position(|option| *option == current)
        .unwrap_or(0) as i32;
    let len = options.len() as i32;
    let next_index = (current_index + delta).rem_euclid(len) as usize;
    options[next_index].to_string()
}

fn adjust_font_size_value(current: f32, delta: i32) -> f32 {
    (current + delta as f32).clamp(6.0, 72.0)
}

#[cfg(target_os = "windows")]
fn pick_background_image_file() -> Option<PathBuf> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

    std::thread::spawn(|| {
        let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        let dialog_result = std::panic::catch_unwind(|| {
            rfd::FileDialog::new()
                .add_filter(
                    "Images",
                    &[
                        "png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "svg", "avif", "ico",
                    ],
                )
                .pick_file()
        })
        .ok()
        .flatten();
        if com_initialized {
            unsafe { CoUninitialize() };
        }
        dialog_result
    })
    .join()
    .ok()
    .flatten()
}

#[cfg(not(target_os = "windows"))]
fn pick_background_image_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(
            "Images",
            &[
                "png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "svg", "avif", "ico",
            ],
        )
        .pick_file()
}

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
    terminal_input_suppressed: Arc<AtomicBool>,
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
            (Self::General, "一般", "ui/settings-general.svg"),
            (Self::Appearance, "外観", "ui/settings-appearance.svg"),
            (Self::Profiles, "プロファイル", "ui/settings-terminal.svg"),
            (Self::Keyboard, "キーボード", "ui/settings-key.svg"),
            (Self::Notifications, "通知", "ui/settings-notifications.svg"),
            (Self::Privacy, "プライバシー", "ui/settings-privacy.svg"),
            (Self::Advanced, "詳細", "ui/settings-advanced.svg"),
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
enum SnippetSettingsCategory {
    General,
    Hotkeys,
    Display,
    Filters,
    Templates,
    Notifications,
    Data,
    Advanced,
}

impl SnippetSettingsCategory {
    fn all() -> &'static [SnippetSettingsCategory] {
        &[
            Self::General,
            Self::Hotkeys,
            Self::Display,
            Self::Filters,
            Self::Templates,
            Self::Notifications,
            Self::Data,
            Self::Advanced,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            Self::General => "一般",
            Self::Hotkeys => "ホットキー",
            Self::Display => "表示",
            Self::Filters => "フィルタ",
            Self::Templates => "定型文",
            Self::Notifications => "通知",
            Self::Data => "データ管理",
            Self::Advanced => "詳細設定",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnippetSettingsTextField {
    ShowWindowHotkey,
    PasteAsPlainTextHotkey,
    QuickPasteHotkey,
    ShowFavoritesHotkey,
    ShowTemplatesHotkey,
    FilterPatternInput,
    ExcludeApps,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyboardSettingsTextField {
    NewTab,
    CloseTab,
    SplitRight,
    SplitDown,
    ClosePane,
    FocusNextPane,
    FocusPrevPane,
    OpenSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobalShortcutAction {
    NewTab,
    CloseTab,
    SplitRight,
    SplitDown,
    ClosePane,
    FocusNext,
    FocusPrev,
    OpenSettings,
    ToggleSnippetPalette,
    SnippetQueuePaste,
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AiSettingsTextField {
    ApiKey,
    Model,
}

#[derive(Clone)]
struct SnippetNotice {
    title: String,
    detail: String,
    created_at: Instant,
    duration_ms: u64,
}

#[derive(Clone)]
struct AppNotice {
    title: String,
    detail: String,
    created_at: Instant,
    duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnippetGroupEditMode {
    None,
    Rename(usize),
    New(usize),
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

fn begin_new_snippet_group_edit(
    palette: &mut SnippetPalette,
    editor: &mut SnippetGroupEditorState,
) -> bool {
    if !palette.create_group() {
        return false;
    }

    let created_index = palette
        .active_group()
        .and_then(|group| palette.group_index_by_name(group))
        .unwrap_or_else(|| palette.groups().len().saturating_sub(1));
    let created_name = palette
        .group_name_at(created_index)
        .unwrap_or("")
        .to_string();

    editor.selected_index = created_index;
    editor.edit_mode = SnippetGroupEditMode::New(created_index);
    editor.draft_name = created_name;
    true
}

fn cancel_snippet_group_edit(palette: &mut SnippetPalette, editor: &mut SnippetGroupEditorState) {
    let edit_mode = editor.edit_mode;
    editor.edit_mode = SnippetGroupEditMode::None;
    editor.draft_name.clear();

    if let SnippetGroupEditMode::New(index) = edit_mode {
        if palette.delete_group(index) {
            editor.selected_index = index
                .saturating_sub(1)
                .min(palette.groups().len().saturating_sub(1));
        }
    }
}

fn apply_snippet_group_edit(
    palette: &mut SnippetPalette,
    editor: &mut SnippetGroupEditorState,
) -> bool {
    let draft_name = editor.draft_name.trim().to_string();
    match editor.edit_mode {
        SnippetGroupEditMode::None => false,
        SnippetGroupEditMode::Rename(index) => {
            if draft_name.is_empty() {
                editor.edit_mode = SnippetGroupEditMode::None;
                editor.draft_name.clear();
                return false;
            }

            if !palette.rename_group(index, draft_name) {
                return false;
            }

            editor.selected_index = index;
            editor.edit_mode = SnippetGroupEditMode::None;
            editor.draft_name.clear();
            true
        }
        SnippetGroupEditMode::New(index) => {
            if draft_name.is_empty() {
                cancel_snippet_group_edit(palette, editor);
                return true;
            }

            let current_name = palette.group_name_at(index).unwrap_or("").to_string();
            if !current_name.eq_ignore_ascii_case(&draft_name)
                && !palette.rename_group(index, draft_name)
            {
                return false;
            }

            editor.selected_index = palette
                .active_group()
                .and_then(|group| palette.group_index_by_name(group))
                .unwrap_or_else(|| index.min(palette.groups().len().saturating_sub(1)));
            editor.edit_mode = SnippetGroupEditMode::None;
            editor.draft_name.clear();
            true
        }
    }
}

fn append_text_to_snippet_editor_field(editor: &mut SnippetEditorState, text: &str) -> bool {
    match editor.active_field {
        SnippetEditorField::Title => {
            insert_text_at_cursor(&mut editor.title, &mut editor.title_cursor, text)
        }
        SnippetEditorField::Content => {
            insert_text_at_cursor(&mut editor.content, &mut editor.content_cursor, text)
        }
    }
}

fn paste_text_to_snippet_editor_field(editor: &mut SnippetEditorState, text: &str) -> bool {
    let normalized = normalize_pasted_snippet_editor_text(editor.active_field, text);
    if normalized.is_empty() {
        return false;
    }

    append_text_to_snippet_editor_field(editor, &normalized)
}

fn delete_text_before_snippet_editor_cursor(editor: &mut SnippetEditorState) -> bool {
    match editor.active_field {
        SnippetEditorField::Title => {
            delete_char_before_cursor(&mut editor.title, &mut editor.title_cursor)
        }
        SnippetEditorField::Content => {
            delete_char_before_cursor(&mut editor.content, &mut editor.content_cursor)
        }
    }
}

fn move_snippet_editor_cursor(editor: &mut SnippetEditorState, movement: TextCursorMove) -> bool {
    match editor.active_field {
        SnippetEditorField::Title => {
            move_cursor_in_text(&editor.title, &mut editor.title_cursor, movement)
        }
        SnippetEditorField::Content => {
            move_cursor_in_text(&editor.content, &mut editor.content_cursor, movement)
        }
    }
}

fn active_snippet_editor_text_with_cursor(editor: &SnippetEditorState) -> (String, String) {
    (
        text_with_cursor_marker(
            &editor.title,
            editor.title_cursor,
            editor.active_field == SnippetEditorField::Title,
        ),
        text_with_cursor_marker(
            &editor.content,
            editor.content_cursor,
            editor.active_field == SnippetEditorField::Content,
        ),
    )
}

fn normalize_pasted_snippet_editor_text(field: SnippetEditorField, text: &str) -> String {
    let normalized_newlines = text.replace("\r\n", "\n").replace('\r', "\n");
    match field {
        SnippetEditorField::Title => normalized_newlines.replace('\n', " "),
        SnippetEditorField::Content => normalized_newlines,
    }
}

fn insert_text_at_cursor(text: &mut String, cursor: &mut usize, input: &str) -> bool {
    if input.is_empty() {
        return false;
    }

    let insertion_index = (*cursor).min(text.len());
    text.insert_str(insertion_index, input);
    *cursor = insertion_index + input.len();
    true
}

fn delete_char_before_cursor(text: &mut String, cursor: &mut usize) -> bool {
    if *cursor == 0 || text.is_empty() {
        return false;
    }

    let end = (*cursor).min(text.len());
    let start = previous_char_boundary(text, end);
    if start == end {
        return false;
    }

    text.replace_range(start..end, "");
    *cursor = start;
    true
}

fn move_cursor_in_text(text: &str, cursor: &mut usize, movement: TextCursorMove) -> bool {
    let current = (*cursor).min(text.len());
    let next = match movement {
        TextCursorMove::Left => previous_char_boundary(text, current),
        TextCursorMove::Right => next_char_boundary(text, current),
        TextCursorMove::Home => 0,
        TextCursorMove::End => text.len(),
    };

    if next == current {
        return false;
    }

    *cursor = next;
    true
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    if cursor >= text.len() {
        return text.len();
    }

    text[cursor..]
        .chars()
        .next()
        .map(|ch| cursor + ch.len_utf8())
        .unwrap_or(text.len())
}

fn text_with_cursor_marker(text: &str, cursor: usize, active: bool) -> String {
    if !active {
        return text.to_string();
    }

    let cursor = cursor.min(text.len());
    let (head, tail) = text.split_at(cursor);
    let mut rendered = String::with_capacity(text.len() + 1);
    rendered.push_str(head);
    rendered.push('|');
    rendered.push_str(tail);
    rendered
}

fn byte_range_to_utf16_range(text: &str, range: &Range<usize>) -> Range<usize> {
    byte_index_to_utf16_offset(text, range.start)..byte_index_to_utf16_offset(text, range.end)
}

fn utf16_range_to_byte_range(text: &str, range: &Range<usize>) -> Range<usize> {
    utf16_offset_to_byte_index(text, range.start)..utf16_offset_to_byte_index(text, range.end)
}

fn byte_index_to_utf16_offset(text: &str, byte_index: usize) -> usize {
    text[..byte_index.min(text.len())].encode_utf16().count()
}

fn utf16_offset_to_byte_index(text: &str, utf16_offset: usize) -> usize {
    if utf16_offset == 0 {
        return 0;
    }

    let mut consumed = 0;
    for (byte_index, ch) in text.char_indices() {
        let next = consumed + ch.len_utf16();
        if next > utf16_offset {
            return byte_index;
        }
        consumed = next;
        if consumed == utf16_offset {
            return byte_index + ch.len_utf8();
        }
    }

    text.len()
}

fn append_text_to_snippet_ime_target(
    target: SnippetImeTarget,
    palette: &mut SnippetPalette,
    group_editor: Option<&mut SnippetGroupEditorState>,
    snippet_editor: Option<&mut SnippetEditorState>,
    text: &str,
) -> bool {
    if text.is_empty() {
        return false;
    }

    match target {
        SnippetImeTarget::PaletteQuery => {
            let mut query = palette.query().to_string();
            query.push_str(text);
            palette.set_query(query);
            true
        }
        SnippetImeTarget::GroupName => {
            let Some(editor) = group_editor else {
                return false;
            };
            if editor.edit_mode == SnippetGroupEditMode::None {
                return false;
            }
            editor.draft_name.push_str(text);
            true
        }
        SnippetImeTarget::EditorTitle => {
            let Some(editor) = snippet_editor else {
                return false;
            };
            insert_text_at_cursor(&mut editor.title, &mut editor.title_cursor, text)
        }
        SnippetImeTarget::EditorContent => {
            let Some(editor) = snippet_editor else {
                return false;
            };
            insert_text_at_cursor(&mut editor.content, &mut editor.content_cursor, text)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnippetImeTarget {
    PaletteQuery,
    GroupName,
    EditorTitle,
    EditorContent,
}

#[derive(Clone)]
struct SnippetEditorState {
    mode: SnippetEditorMode,
    title: String,
    title_cursor: usize,
    content: String,
    content_cursor: usize,
    group_index: usize,
    active_field: SnippetEditorField,
}

#[derive(Clone, Copy)]
enum TextCursorMove {
    Left,
    Right,
    Home,
    End,
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
    snippet_input_focus_handle: FocusHandle,
    show_shell_menu: bool,
    show_settings: bool,
    show_snippet_settings: bool,
    show_close_confirm: bool,
    show_snippet_context_menu: bool,
    snippet_palette: SnippetPalette,
    ai_client: AiDirectClient,
    ai_suggestion_state: AiSuggestionState,
    snippet_settings: SnippetSettings,
    snippet_settings_draft: Option<SnippetSettings>,
    snippet_settings_category: SnippetSettingsCategory,
    snippet_settings_active_text: Option<SnippetSettingsTextField>,
    keyboard_settings_active_text: Option<KeyboardSettingsTextField>,
    ai_settings_active_text: Option<AiSettingsTextField>,
    snippet_notice: Option<SnippetNotice>,
    app_notice: Option<AppNotice>,
    snippet_group_editor: Option<SnippetGroupEditorState>,
    snippet_list_editor: Option<SnippetListEditorState>,
    snippet_editor: Option<SnippetEditorState>,
    snippet_csv_dialog: Option<SnippetCsvDialogState>,
    snippet_ime_target: Option<SnippetImeTarget>,
    snippet_ime_marked_range: Option<Range<usize>>,
    snippet_ime_selected_range: Option<Range<usize>>,
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

        let terminal_input_suppressed = Arc::new(AtomicBool::new(false));
        let (shell_type, title) = shell_meta_for_command(&default_shell, &available_shells);
        let terminal_settings =
            terminal_settings_from_config(&config, terminal_input_suppressed.clone());
        let split = cx.new(|cx| SplitContainer::new(&default_shell, terminal_settings, cx));
        bind_global_key_bindings(cx, &config, false);

        Self {
            tabs: vec![Tab {
                title,
                shell_type,
                split,
            }],
            active_tab: 0,
            available_shells,
            config,
            terminal_input_suppressed,
        }
    }

    pub fn add_tab(&mut self, cx: &mut App) {
        let shell = resolve_default_shell_command(&self.config.shell, &self.available_shells);
        self.add_tab_with_shell(&shell, cx);
    }

    pub fn add_tab_with_shell(&mut self, shell: &str, cx: &mut App) {
        let (shell_type, title) = shell_meta_for_command(shell, &self.available_shells);
        let terminal_settings =
            terminal_settings_from_config(&self.config, self.terminal_input_suppressed.clone());
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
        bind_global_key_bindings(cx, &self.config, false);
        let terminal_settings =
            terminal_settings_from_config(&self.config, self.terminal_input_suppressed.clone());
        for tab in &self.tabs {
            let terminal_settings = terminal_settings.clone();
            tab.split.update(cx, |split, cx| {
                split.update_terminal_settings(terminal_settings, cx);
            });
        }
    }
}

impl RootView {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        install_snippet_ime_hook();

        let launch_on_startup = state.read(_cx).config.launch_on_login;
        let mut snippet_settings = SnippetSettings::load();
        snippet_settings.general.launch_on_startup = launch_on_startup;
        let mut snippet_palette = SnippetPalette::load();
        snippet_palette.set_filter_preferences(
            snippet_settings.filters.ignore_case,
            snippet_settings.filters.min_text_length,
            snippet_settings.filters.exclude_patterns.clone(),
        );
        if snippet_settings.advanced.fifo_mode {
            snippet_palette.set_queue_mode(SnippetQueueMode::Fifo);
        }

        Self {
            state,
            focus_handle: _cx.focus_handle(),
            snippet_input_focus_handle: _cx.focus_handle(),
            show_shell_menu: false,
            show_settings: false,
            show_snippet_settings: false,
            show_close_confirm: false,
            show_snippet_context_menu: false,
            snippet_palette,
            ai_client: AiDirectClient::default(),
            ai_suggestion_state: AiSuggestionState::default(),
            snippet_settings,
            snippet_settings_draft: None,
            snippet_settings_category: SnippetSettingsCategory::General,
            snippet_settings_active_text: None,
            keyboard_settings_active_text: None,
            ai_settings_active_text: None,
            snippet_notice: None,
            app_notice: None,
            snippet_group_editor: None,
            snippet_list_editor: None,
            snippet_editor: None,
            snippet_csv_dialog: None,
            snippet_ime_target: None,
            snippet_ime_marked_range: None,
            snippet_ime_selected_range: None,
            settings_category: SettingsCategory::General,
            last_bounds: None,
            last_save_time: Instant::now(),
            bounds_dirty: false,
        }
    }

    fn compute_snippet_ime_target(&self) -> Option<SnippetImeTarget> {
        if let Some(editor) = self.snippet_editor.as_ref() {
            return Some(match editor.active_field {
                SnippetEditorField::Title => SnippetImeTarget::EditorTitle,
                SnippetEditorField::Content => SnippetImeTarget::EditorContent,
            });
        }

        if self
            .snippet_group_editor
            .as_ref()
            .map(|editor| editor.edit_mode != SnippetGroupEditMode::None)
            .unwrap_or(false)
        {
            return Some(SnippetImeTarget::GroupName);
        }

        if self.snippet_palette.is_visible()
            && self.snippet_group_editor.is_none()
            && self.snippet_list_editor.is_none()
            && self.snippet_editor.is_none()
            && self.snippet_csv_dialog.is_none()
            && !self.show_snippet_settings
            && !self.show_snippet_context_menu
        {
            return Some(SnippetImeTarget::PaletteQuery);
        }

        None
    }

    fn sync_snippet_ime_target(&mut self) {
        let target = self.compute_snippet_ime_target();
        if self.snippet_ime_target != target {
            self.snippet_ime_target = target;
            self.snippet_ime_marked_range = None;
            self.snippet_ime_selected_range = None;
        }
    }

    fn clear_snippet_ime_state(&mut self) {
        self.snippet_ime_marked_range = None;
        self.snippet_ime_selected_range = None;
    }

    fn current_snippet_ime_text(&self, target: SnippetImeTarget) -> Option<String> {
        match target {
            SnippetImeTarget::PaletteQuery => Some(self.snippet_palette.query().to_string()),
            SnippetImeTarget::GroupName => self
                .snippet_group_editor
                .as_ref()
                .filter(|editor| editor.edit_mode != SnippetGroupEditMode::None)
                .map(|editor| editor.draft_name.clone()),
            SnippetImeTarget::EditorTitle => self
                .snippet_editor
                .as_ref()
                .map(|editor| editor.title.clone()),
            SnippetImeTarget::EditorContent => self
                .snippet_editor
                .as_ref()
                .map(|editor| editor.content.clone()),
        }
    }

    fn current_snippet_ime_selection_range(
        &self,
        target: SnippetImeTarget,
    ) -> Option<Range<usize>> {
        if let Some(range) = self.snippet_ime_selected_range.clone() {
            return Some(range);
        }

        match target {
            SnippetImeTarget::PaletteQuery => {
                let len = self.snippet_palette.query().len();
                Some(len..len)
            }
            SnippetImeTarget::GroupName => self
                .snippet_group_editor
                .as_ref()
                .filter(|editor| editor.edit_mode != SnippetGroupEditMode::None)
                .map(|editor| {
                    let len = editor.draft_name.len();
                    len..len
                }),
            SnippetImeTarget::EditorTitle => self.snippet_editor.as_ref().map(|editor| {
                editor.title_cursor.min(editor.title.len())
                    ..editor.title_cursor.min(editor.title.len())
            }),
            SnippetImeTarget::EditorContent => self.snippet_editor.as_ref().map(|editor| {
                editor.content_cursor.min(editor.content.len())
                    ..editor.content_cursor.min(editor.content.len())
            }),
        }
    }

    fn replace_text_in_active_snippet_ime_target(
        &mut self,
        target: SnippetImeTarget,
        range: Range<usize>,
        text: &str,
    ) -> Option<Range<usize>> {
        match target {
            SnippetImeTarget::PaletteQuery => {
                let mut query = self.snippet_palette.query().to_string();
                let range = range.start.min(query.len())..range.end.min(query.len());
                query.replace_range(range.clone(), text);
                let inserted = range.start..range.start + text.len();
                self.snippet_palette.set_query(query);
                Some(inserted)
            }
            SnippetImeTarget::GroupName => {
                let editor = self.snippet_group_editor.as_mut()?;
                if editor.edit_mode == SnippetGroupEditMode::None {
                    return None;
                }
                let range = range.start.min(editor.draft_name.len())
                    ..range.end.min(editor.draft_name.len());
                editor.draft_name.replace_range(range.clone(), text);
                Some(range.start..range.start + text.len())
            }
            SnippetImeTarget::EditorTitle => {
                let editor = self.snippet_editor.as_mut()?;
                let range = range.start.min(editor.title.len())..range.end.min(editor.title.len());
                editor.title.replace_range(range.clone(), text);
                editor.title_cursor = range.start + text.len();
                Some(range.start..range.start + text.len())
            }
            SnippetImeTarget::EditorContent => {
                let editor = self.snippet_editor.as_mut()?;
                let range =
                    range.start.min(editor.content.len())..range.end.min(editor.content.len());
                editor.content.replace_range(range.clone(), text);
                editor.content_cursor = range.start + text.len();
                Some(range.start..range.start + text.len())
            }
        }
    }

    fn append_active_snippet_ime_text(&mut self, text: &str) -> bool {
        let Some(target) = self.compute_snippet_ime_target() else {
            return false;
        };

        append_text_to_snippet_ime_target(
            target,
            &mut self.snippet_palette,
            self.snippet_group_editor.as_mut(),
            self.snippet_editor.as_mut(),
            text,
        )
    }

    fn request_ai_suggestions(&mut self, cx: &mut Context<Self>) {
        let config = self.state.read(cx).config.clone();
        let provider = sanitize_ai_provider(&config.ai_provider);
        let api_key = resolve_ai_api_key(provider, &config.ai_api_key);
        let model = resolve_ai_model(provider, &config.ai_model);
        let request = self.ai_suggestion_state.begin_request(
            self.snippet_palette.query(),
            config.ai_suggestions_enabled,
            api_key.is_some(),
            provider,
            &model,
        );
        cx.notify();

        let Some(request) = request else {
            return;
        };
        let Some(api_key) = api_key else {
            return;
        };

        let client = self.ai_client.clone();
        cx.spawn(async move |this: WeakEntity<RootView>, cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(Duration::from_millis(AI_SUGGEST_DEBOUNCE_MS))
                .await;

            let result = cx
                .background_executor()
                .spawn(async move {
                    client.fetch_suggestions(
                        request.provider,
                        &api_key,
                        &request.query,
                        &request.model,
                    )
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view
                    .ai_suggestion_state
                    .apply_result(request.request_id, result)
                {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn update_palette_query_and_ai(&mut self, query: impl Into<String>, cx: &mut Context<Self>) {
        self.snippet_palette.set_query(query);
        self.request_ai_suggestions(cx);
    }

    fn clear_palette_query_and_ai(&mut self, cx: &mut Context<Self>) {
        self.snippet_palette.clear_query();
        self.request_ai_suggestions(cx);
    }

    fn activate_ai_suggestion(
        &mut self,
        suggestion: AiSuggestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = suggestion.command;
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            if let Some(terminal) = split.read(cx).focused_terminal() {
                let _ = terminal.read(cx).send_input(content.as_bytes());
            }
        }

        self.play_snippet_sound();
        if self.snippet_settings.notifications.show_paste_notification {
            self.show_snippet_notice(
                "AI コマンドを送信しました",
                suggestion.title,
                self.snippet_settings.notifications.notification_duration,
                cx,
            );
        }
        self.snippet_palette.hide();
        self.focus_active_terminal(window, cx);
        cx.notify();
    }

    fn ai_suggestion_status_message(&self, provider: AiProvider) -> Option<String> {
        match self.ai_suggestion_state.status() {
            AiSuggestionStatus::Idle => {
                if self.snippet_palette.query().trim().chars().count() >= 3 {
                    Some("AI サジェストを待機中です".to_string())
                } else {
                    None
                }
            }
            AiSuggestionStatus::Loading => Some(format!("{} に問い合わせ中...", provider.label())),
            AiSuggestionStatus::Ready => {
                if self.ai_suggestion_state.suggestions().is_empty() {
                    Some("AI サジェストは見つかりませんでした".to_string())
                } else {
                    None
                }
            }
            AiSuggestionStatus::Disabled => Some("AI サジェストは無効です".to_string()),
            AiSuggestionStatus::MissingApiKey => Some(format!(
                "{} の API キーを設定するとサジェストを表示できます ({})",
                provider.label(),
                provider.api_env()
            )),
            AiSuggestionStatus::Error(error) => Some(format!("AI サジェスト取得失敗: {error}")),
        }
    }

    fn open_snippet_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut draft = self.snippet_settings.clone();
        draft.general.launch_on_startup = self.state.read(cx).config.launch_on_login;
        self.show_snippet_settings = true;
        self.show_settings = false;
        self.show_shell_menu = false;
        self.show_snippet_context_menu = false;
        self.show_close_confirm = false;
        self.snippet_palette.hide();
        self.snippet_settings_category = SnippetSettingsCategory::General;
        self.snippet_settings_active_text = None;
        self.snippet_settings_draft = Some(draft);
        window.focus(&self.focus_handle);
    }

    fn close_snippet_settings(&mut self, save: bool, cx: &mut Context<Self>) {
        if save {
            if let Some(draft) = self.snippet_settings_draft.take() {
                self.snippet_settings = draft.validated();
                self.apply_snippet_settings(cx);
                if let Err(err) = self.snippet_settings.save() {
                    log::warn!("Failed to save snippet settings: {}", err);
                    self.show_snippet_notice("設定の保存に失敗しました", err.to_string(), 2500, cx);
                }
                let config = self.state.read(cx).config.clone();
                self.state.update(cx, |_state, app| {
                    bind_global_key_bindings(app, &config, false);
                });
            }
        } else {
            self.snippet_settings_draft = None;
        }

        self.show_snippet_settings = false;
        self.snippet_settings_active_text = None;
    }

    fn open_settings_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_shell_menu = false;
        self.show_snippet_settings = false;
        self.show_settings = true;
        self.keyboard_settings_active_text = None;
        self.ai_settings_active_text = None;
        self.refresh_global_key_bindings(cx);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn close_settings_panel(&mut self, cx: &mut Context<Self>) {
        self.show_settings = false;
        self.keyboard_settings_active_text = None;
        self.ai_settings_active_text = None;
        self.refresh_global_key_bindings(cx);
        cx.notify();
    }

    fn apply_snippet_settings(&mut self, cx: &mut Context<Self>) {
        self.snippet_palette.set_filter_preferences(
            self.snippet_settings.filters.ignore_case,
            self.snippet_settings.filters.min_text_length,
            self.snippet_settings.filters.exclude_patterns.clone(),
        );
        self.snippet_palette
            .set_queue_mode(if self.snippet_settings.advanced.fifo_mode {
                SnippetQueueMode::Fifo
            } else if self.snippet_palette.queue_mode() == SnippetQueueMode::Fifo {
                SnippetQueueMode::Off
            } else {
                self.snippet_palette.queue_mode()
            });

        let launch_on_login = self.snippet_settings.general.launch_on_startup;
        self.persist_config_update(cx, |config| {
            config.launch_on_login = launch_on_login;
        });
        if let Err(err) = set_launch_on_login(launch_on_login) {
            log::warn!(
                "Failed to update launch-on-login setting from snippet settings: {}",
                err
            );
        }
    }

    fn show_snippet_notice(
        &mut self,
        title: impl Into<String>,
        detail: impl Into<String>,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        self.snippet_notice = Some(SnippetNotice {
            title: title.into(),
            detail: detail.into(),
            created_at: Instant::now(),
            duration_ms,
        });
        cx.notify();
    }

    fn maybe_clear_snippet_notice(&mut self) {
        if self
            .snippet_notice
            .as_ref()
            .map(|notice| notice.created_at.elapsed().as_millis() >= notice.duration_ms as u128)
            .unwrap_or(false)
        {
            self.snippet_notice = None;
        }
    }

    fn show_app_notice(
        &mut self,
        title: impl Into<String>,
        detail: impl Into<String>,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        self.app_notice = Some(AppNotice {
            title: title.into(),
            detail: detail.into(),
            created_at: Instant::now(),
            duration_ms,
        });
        cx.notify();
    }

    fn maybe_clear_app_notice(&mut self) {
        if self
            .app_notice
            .as_ref()
            .map(|notice| notice.created_at.elapsed().as_millis() >= notice.duration_ms as u128)
            .unwrap_or(false)
        {
            self.app_notice = None;
        }
    }

    fn process_terminal_notifications(&mut self, cx: &mut Context<Self>) {
        let (notifications_enabled, visual_bell, bell_enabled, tabs) = {
            let state = self.state.read(cx);
            (
                state.config.process_completion_alert,
                state.config.visual_bell,
                state.config.notification_bell,
                state
                    .tabs
                    .iter()
                    .map(|tab| (tab.title.clone(), tab.split.clone()))
                    .collect::<Vec<_>>(),
            )
        };

        if !notifications_enabled {
            return;
        }

        for (tab_title, split) in tabs {
            for (_, terminal) in split.read(cx).all_terminals() {
                let exit_status = terminal.update(cx, |pane, _cx| pane.take_process_exit_status());
                let Some(exit_code) = exit_status else {
                    continue;
                };

                if bell_enabled {
                    play_notification_bell();
                }

                if visual_bell {
                    self.show_app_notice(
                        "処理が完了しました",
                        process_completion_notice_detail(&tab_title, exit_code),
                        2400,
                        cx,
                    );
                }
            }
        }
    }

    fn play_snippet_sound(&self) {
        if !self.snippet_settings.general.play_sound {
            return;
        }
        print!("\x07");
        let _ = std::io::stdout().flush();
    }

    fn update_snippet_settings_draft<F>(&mut self, cx: &mut Context<Self>, mutate: F)
    where
        F: FnOnce(&mut SnippetSettings),
    {
        if let Some(draft) = self.snippet_settings_draft.as_mut() {
            mutate(draft);
            cx.notify();
        }
    }

    fn cycle_snippet_setting(value: &mut String, options: &[&str]) {
        let current = options
            .iter()
            .position(|candidate| value.eq_ignore_ascii_case(candidate))
            .unwrap_or(0);
        *value = options[(current + 1) % options.len()].to_string();
    }

    fn add_snippet_exclude_pattern(&mut self, cx: &mut Context<Self>) {
        self.update_snippet_settings_draft(cx, |draft| {
            if let Some(pattern) = draft.filters.exclude_patterns.last_mut() {
                let trimmed = pattern.trim().to_string();
                if trimmed.is_empty() {
                    draft.filters.exclude_patterns.pop();
                } else {
                    *pattern = trimmed;
                    draft.filters.exclude_patterns.push(String::new());
                }
            } else {
                draft.filters.exclude_patterns.push(String::new());
            }
        });
        self.snippet_settings_active_text = Some(SnippetSettingsTextField::FilterPatternInput);
    }

    fn finalize_snippet_pattern_input(&mut self, cx: &mut Context<Self>) {
        self.update_snippet_settings_draft(cx, |draft| {
            draft.filters.exclude_patterns = draft
                .filters
                .exclude_patterns
                .iter()
                .map(|pattern| pattern.trim())
                .filter(|pattern| !pattern.is_empty())
                .map(str::to_string)
                .collect();
        });
        self.snippet_settings_active_text = None;
    }

    fn remove_snippet_exclude_pattern(&mut self, index: usize, cx: &mut Context<Self>) {
        self.update_snippet_settings_draft(cx, |draft| {
            if index < draft.filters.exclude_patterns.len() {
                draft.filters.exclude_patterns.remove(index);
            }
        });
    }

    fn export_snippet_settings_json(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.snippet_settings_draft.as_ref().cloned() else {
            return;
        };
        let default_name = "zwg-snippet-settings.json";
        let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        match draft.save_to_path(&path) {
            Ok(()) => self.show_snippet_notice(
                "設定を書き出しました",
                path.display().to_string(),
                2200,
                cx,
            ),
            Err(err) => {
                self.show_snippet_notice("設定の書き出しに失敗しました", err.to_string(), 2600, cx)
            }
        }
    }

    fn import_snippet_settings_json(&mut self, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };

        match crate::snippets::SnippetSettings::import_from_path(&path) {
            Ok(imported) => {
                self.snippet_settings_draft = Some(imported);
                self.snippet_settings_active_text = None;
                self.show_snippet_notice(
                    "設定を読み込みました",
                    path.display().to_string(),
                    2200,
                    cx,
                );
                cx.notify();
            }
            Err(err) => {
                self.show_snippet_notice("設定の読込に失敗しました", err.to_string(), 2600, cx)
            }
        }
    }

    fn backup_snippets_to_file(&mut self, cx: &mut Context<Self>) {
        let default_name = "zwg-snippets-backup.json";
        let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };

        match std::fs::copy(self.snippet_palette.store_path(), &path) {
            Ok(_) => self.show_snippet_notice(
                "定型文をバックアップしました",
                path.display().to_string(),
                2200,
                cx,
            ),
            Err(err) => {
                self.show_snippet_notice("バックアップに失敗しました", err.to_string(), 2600, cx)
            }
        }
    }

    fn restore_snippet_backup(&mut self, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };

        match std::fs::copy(&path, self.snippet_palette.store_path()) {
            Ok(_) => {
                self.snippet_palette.reload_from_disk();
                self.show_snippet_notice(
                    "定型文を復元しました",
                    path.display().to_string(),
                    2200,
                    cx,
                );
                cx.notify();
            }
            Err(err) => self.show_snippet_notice("復元に失敗しました", err.to_string(), 2600, cx),
        }
    }

    fn clear_all_snippets(&mut self, cx: &mut Context<Self>) {
        if self.snippet_palette.clear_all() {
            self.show_snippet_notice("全ての定型文を削除しました", "", 1800, cx);
            cx.notify();
        } else {
            self.show_snippet_notice(
                "定型文の削除に失敗しました",
                "snippets.json を確認してください",
                2200,
                cx,
            );
        }
    }

    fn on_new_tab(&mut self, _action: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcut_actions_blocked() {
            return;
        }
        self.show_shell_menu = false;
        self.show_settings = false;
        self.state.update(cx, |state, cx| {
            state.add_tab(cx);
            cx.notify();
        });
        cx.defer_in(window, |this, window, cx| {
            this.focus_active_terminal(window, cx);
        });
    }

    fn on_close_tab(&mut self, _action: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcut_actions_blocked() {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.close_tab(state.active_tab);
            cx.notify();
        });
        cx.defer_in(window, |this, window, cx| {
            this.focus_active_terminal(window, cx);
        });
    }

    fn on_split_right(
        &mut self,
        _action: &SplitRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shortcut_actions_blocked() {
            return;
        }
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.split(SplitDirection::Horizontal, cx));
        }
        cx.defer_in(window, |this, window, cx| {
            this.focus_active_terminal(window, cx);
        });
    }

    fn on_split_down(&mut self, _action: &SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcut_actions_blocked() {
            return;
        }
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.split(SplitDirection::Vertical, cx));
        }
        cx.defer_in(window, |this, window, cx| {
            this.focus_active_terminal(window, cx);
        });
    }

    fn on_close_pane(&mut self, _action: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcut_actions_blocked() {
            return;
        }
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| {
                sc.close_focused(cx);
            });
        }
        cx.defer_in(window, |this, window, cx| {
            this.focus_active_terminal(window, cx);
        });
    }

    fn on_focus_next(&mut self, _action: &FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcut_actions_blocked() {
            return;
        }
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.focus_direction(FocusDir::Next, cx));
        }
        cx.defer_in(window, |this, window, cx| {
            this.focus_active_terminal(window, cx);
        });
    }

    fn on_focus_prev(&mut self, _action: &FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcut_actions_blocked() {
            return;
        }
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            split.update(cx, |sc, cx| sc.focus_direction(FocusDir::Prev, cx));
        }
        cx.defer_in(window, |this, window, cx| {
            this.focus_active_terminal(window, cx);
        });
    }

    fn on_toggle_snippet_palette(
        &mut self,
        _action: &ToggleSnippetPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_shell_menu = false;
        self.show_settings = false;
        self.show_snippet_settings = false;
        self.show_close_confirm = false;
        self.show_snippet_context_menu = false;
        self.snippet_palette.set_favorites_only(false);
        self.snippet_palette.toggle();
        if self.snippet_palette.is_visible() {
            self.request_ai_suggestions(cx);
            window.focus(&self.focus_handle);
        } else {
            self.focus_active_terminal(window, cx);
        }
        cx.notify();
    }

    fn on_open_settings(
        &mut self,
        _action: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_snippet_settings
            || self.snippet_group_editor.is_some()
            || self.snippet_list_editor.is_some()
            || self.snippet_editor.is_some()
            || self.snippet_csv_dialog.is_some()
        {
            return;
        }

        self.open_settings_panel(window, cx);
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
        let config = self.state.read(cx).config.clone();
        let should_confirm = config.confirm_on_close;
        if should_confirm {
            self.show_shell_menu = false;
            self.show_settings = false;
            self.show_snippet_settings = false;
            self.show_snippet_context_menu = false;
            self.snippet_palette.hide();
            self.show_close_confirm = true;
            cx.notify();
        } else {
            if config.clear_history_on_exit {
                self.clear_runtime_history(cx);
            }
            window.remove_window();
        }
    }

    fn minimize_window(&mut self, window: &mut Window) {
        window.minimize_window();
    }

    fn toggle_zoom_window(&mut self, window: &mut Window) {
        match zoom_action_for_window(window.is_maximized()) {
            ZoomAction::Maximize => window.zoom_window(),
            ZoomAction::Restore => {
                if !try_restore_window(window) {
                    window.zoom_window();
                }
            }
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

    fn toggle_notification_bell(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.notification_bell;
        self.persist_config_update(cx, move |config| {
            config.notification_bell = next_value;
        });
    }

    fn toggle_visual_bell(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.visual_bell;
        self.persist_config_update(cx, move |config| {
            config.visual_bell = next_value;
        });
    }

    fn toggle_process_completion_alert(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.process_completion_alert;
        self.persist_config_update(cx, move |config| {
            config.process_completion_alert = next_value;
        });
    }

    fn toggle_copy_on_select(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.copy_on_select;
        self.persist_config_update(cx, move |config| {
            config.copy_on_select = next_value;
        });
    }

    fn toggle_clear_history_on_exit(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.clear_history_on_exit;
        self.persist_config_update(cx, move |config| {
            config.clear_history_on_exit = next_value;
        });
    }

    fn toggle_ai_suggestions_enabled(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.ai_suggestions_enabled;
        self.persist_config_update(cx, move |config| {
            config.ai_suggestions_enabled = next_value;
        });
        self.request_ai_suggestions(cx);
    }

    fn toggle_status_bar_visibility(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.status_bar_visible;
        self.persist_config_update(cx, move |config| {
            config.status_bar_visible = next_value;
        });
    }

    fn toggle_gpu_acceleration(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.gpu_acceleration;
        self.persist_config_update(cx, move |config| {
            config.gpu_acceleration = next_value;
        });
    }

    fn clear_runtime_history(&mut self, cx: &mut Context<Self>) {
        let splits = self
            .state
            .read(cx)
            .tabs
            .iter()
            .map(|tab| tab.split.clone())
            .collect::<Vec<_>>();

        for split in splits {
            for (_, terminal) in split.read(cx).all_terminals() {
                let _ = terminal.update(cx, |pane, cx| {
                    pane.clear_history();
                    cx.notify();
                });
            }
        }

        self.snippet_palette.clear_query();
        self.ai_suggestion_state = AiSuggestionState::default();
        self.snippet_notice = None;
        self.app_notice = None;
    }

    fn reset_app_config(&mut self, cx: &mut Context<Self>) {
        let config = AppConfig::default().sanitized();
        if let Err(err) = set_launch_on_login(config.launch_on_login) {
            log::warn!("Failed to reset launch-on-login setting: {}", err);
        }
        self.state.update(cx, |state, cx| {
            state.apply_config(config.clone(), cx);
            if let Err(err) = state.config.save() {
                log::warn!("Failed to save reset config: {}", err);
            }
            cx.notify();
        });
        self.show_app_notice(
            "設定を初期化しました",
            "アプリ設定を既定値に戻しました。",
            2200,
            cx,
        );
    }

    fn set_theme(&mut self, theme: &'static str, cx: &mut Context<Self>) {
        self.persist_config_update(cx, move |config| {
            config.theme = theme.to_string();
        });
    }

    fn cycle_font_family(&mut self, delta: i32, cx: &mut Context<Self>) {
        let current_family = self.state.read(cx).config.font.family.clone();
        let next_family = cycle_string_option(&current_family, &APPEARANCE_FONT_FAMILIES, delta);
        self.persist_config_update(cx, move |config| {
            config.font.family = next_family;
        });
    }

    fn adjust_font_size(&mut self, delta: i32, cx: &mut Context<Self>) {
        let current_size = self.state.read(cx).config.font.size;
        let next_size = adjust_font_size_value(current_size, delta);
        self.persist_config_update(cx, move |config| {
            config.font.size = next_size;
        });
    }

    fn toggle_cursor_blink(&mut self, cx: &mut Context<Self>) {
        let next_value = !self.state.read(cx).config.cursor_blink;
        self.persist_config_update(cx, move |config| {
            config.cursor_blink = next_value;
        });
    }

    fn pick_background_image(&mut self, cx: &mut Context<Self>) {
        let Some(path) = pick_background_image_file() else {
            return;
        };

        let selected_path = path.display().to_string();
        self.persist_config_update(cx, move |config| {
            config.background_image_path = Some(selected_path);
        });
    }

    fn clear_background_image(&mut self, cx: &mut Context<Self>) {
        self.persist_config_update(cx, |config| {
            config.background_image_path = None;
        });
    }

    fn adjust_background_image_opacity(&mut self, delta: i32, cx: &mut Context<Self>) {
        let current_opacity = self.state.read(cx).config.background_image_opacity as i32;
        let next_opacity = (current_opacity + delta).clamp(0, 100) as u8;
        self.persist_config_update(cx, move |config| {
            config.background_image_opacity = next_opacity;
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

    fn set_keyboard_shortcut(
        &mut self,
        field: KeyboardSettingsTextField,
        shortcut: String,
        cx: &mut Context<Self>,
    ) {
        self.keyboard_settings_active_text = None;
        self.persist_config_update(cx, move |config| match field {
            KeyboardSettingsTextField::NewTab => config.keyboard.new_tab = shortcut,
            KeyboardSettingsTextField::CloseTab => config.keyboard.close_tab = shortcut,
            KeyboardSettingsTextField::SplitRight => config.keyboard.split_right = shortcut,
            KeyboardSettingsTextField::SplitDown => config.keyboard.split_down = shortcut,
            KeyboardSettingsTextField::ClosePane => config.keyboard.close_pane = shortcut,
            KeyboardSettingsTextField::FocusNextPane => config.keyboard.focus_next_pane = shortcut,
            KeyboardSettingsTextField::FocusPrevPane => config.keyboard.focus_prev_pane = shortcut,
            KeyboardSettingsTextField::OpenSettings => config.keyboard.open_settings = shortcut,
        });
    }

    fn set_keyboard_shortcut_capture(
        &mut self,
        field: Option<KeyboardSettingsTextField>,
        cx: &mut Context<Self>,
    ) {
        self.keyboard_settings_active_text = field;
        self.refresh_global_key_bindings(cx);
        cx.notify();
    }

    fn refresh_global_key_bindings(&self, cx: &mut Context<Self>) {
        let config = self.state.read(cx).config.clone();
        let capture_active = self.keyboard_settings_active_text.is_some();
        self.state.update(cx, |_state, app| {
            bind_global_key_bindings(app, &config, capture_active);
        });
    }

    fn set_ai_settings_text(
        &mut self,
        field: AiSettingsTextField,
        value: String,
        cx: &mut Context<Self>,
    ) {
        self.persist_config_update(cx, move |config| match field {
            AiSettingsTextField::ApiKey => config.ai_api_key = value,
            AiSettingsTextField::Model => config.ai_model = value,
        });
        self.request_ai_suggestions(cx);
    }

    fn append_ai_settings_text(
        &mut self,
        field: AiSettingsTextField,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let mut value = match field {
            AiSettingsTextField::ApiKey => self.state.read(cx).config.ai_api_key.clone(),
            AiSettingsTextField::Model => self.state.read(cx).config.ai_model.clone(),
        };
        value.push_str(text);
        self.set_ai_settings_text(field, value, cx);
    }

    fn pop_ai_settings_text(&mut self, field: AiSettingsTextField, cx: &mut Context<Self>) {
        let mut value = match field {
            AiSettingsTextField::ApiKey => self.state.read(cx).config.ai_api_key.clone(),
            AiSettingsTextField::Model => self.state.read(cx).config.ai_model.clone(),
        };
        value.pop();
        self.set_ai_settings_text(field, value, cx);
    }

    fn cycle_ai_provider(&mut self, cx: &mut Context<Self>) {
        let current_config = self.state.read(cx).config.clone();
        let current_provider = sanitize_ai_provider(&current_config.ai_provider);
        let next_provider = next_ai_provider(current_provider);
        let current_default_model = default_model_for_provider(current_provider);
        let should_roll_model = current_config.ai_model.trim().is_empty()
            || current_config.ai_model == current_default_model;
        self.persist_config_update(cx, move |config| {
            config.ai_provider = next_provider.config_value().to_string();
            if should_roll_model {
                config.ai_model = default_model_for_provider(next_provider).to_string();
            }
        });
        self.request_ai_suggestions(cx);
    }

    fn shortcut_actions_blocked(&self) -> bool {
        self.show_settings
            || self.show_snippet_settings
            || self.show_close_confirm
            || self.show_snippet_context_menu
            || self.snippet_palette.is_visible()
            || self.snippet_group_editor.is_some()
            || self.snippet_list_editor.is_some()
            || self.snippet_editor.is_some()
            || self.snippet_csv_dialog.is_some()
    }

    pub fn focus_active_terminal(&self, window: &mut Window, cx: &Context<Self>) {
        let split = self.state.read(cx).active_split().cloned();
        if let Some(split) = split {
            if let Some(terminal) = split.read(cx).focused_terminal() {
                terminal.read(cx).focus_handle(cx).focus(window);
            }
        }
    }

    fn sync_terminal_input_suppression(&self, cx: &Context<Self>) {
        let suppressed = self.show_shell_menu
            || self.show_settings
            || self.show_snippet_settings
            || self.show_close_confirm
            || self.show_snippet_context_menu
            || self.snippet_palette.is_visible()
            || self.snippet_group_editor.is_some()
            || self.snippet_list_editor.is_some()
            || self.snippet_editor.is_some()
            || self.snippet_csv_dialog.is_some();
        self.state
            .read(cx)
            .terminal_input_suppressed
            .store(suppressed, Ordering::Relaxed);
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

        self.play_snippet_sound();
        if self.snippet_settings.notifications.show_paste_notification {
            let preview = content.lines().next().unwrap_or("").trim().to_string();
            self.show_snippet_notice(
                "定型文を送信しました",
                if preview.is_empty() {
                    "空の内容でした".to_string()
                } else {
                    preview
                },
                self.snippet_settings.notifications.notification_duration,
                cx,
            );
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

    fn activate_snippet_store_index(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.snippet_palette.activate_store_index(index) {
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

        self.play_snippet_sound();
        if self.snippet_settings.notifications.show_paste_notification {
            let preview = content.lines().next().unwrap_or("").trim().to_string();
            self.show_snippet_notice(
                "キューから送信しました",
                if preview.is_empty() {
                    "空の内容でした".to_string()
                } else {
                    preview
                },
                self.snippet_settings.notifications.notification_duration,
                cx,
            );
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

    fn open_snippet_group_editor(&mut self, window: &mut Window) {
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
        window.focus(&self.focus_handle);
    }

    fn open_new_snippet_group_editor(&mut self, window: &mut Window) {
        self.open_snippet_group_editor(window);
        if let Some(editor) = self.snippet_group_editor.as_mut() {
            let _ = begin_new_snippet_group_edit(&mut self.snippet_palette, editor);
        }
    }

    fn open_snippet_list_editor(&mut self, window: &mut Window) {
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
        window.focus(&self.focus_handle);
    }

    fn open_snippet_editor(
        &mut self,
        mode: SnippetEditorMode,
        group_hint: Option<usize>,
        window: &mut Window,
    ) {
        let state = match mode {
            SnippetEditorMode::Add => SnippetEditorState {
                mode,
                title: String::new(),
                title_cursor: 0,
                content: String::new(),
                content_cursor: 0,
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
                    title_cursor: snippet.title.len(),
                    content_cursor: snippet.content.len(),
                    title: snippet.title,
                    content: snippet.content,
                    group_index,
                    active_field: SnippetEditorField::Title,
                }
            }
        };

        self.snippet_editor = Some(state);
        window.focus(&self.snippet_input_focus_handle);
    }

    fn open_snippet_csv_dialog(&mut self, window: &mut Window) {
        self.close_snippet_dialogs();
        self.snippet_palette.hide();
        self.snippet_csv_dialog = Some(SnippetCsvDialogState::default());
        window.focus(&self.focus_handle);
    }

    fn close_snippet_group_editor(&mut self) {
        if let Some(editor) = self.snippet_group_editor.as_mut() {
            cancel_snippet_group_edit(&mut self.snippet_palette, editor);
        }
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
        let _ = apply_snippet_group_edit(&mut self.snippet_palette, editor);
    }

    fn save_snippet_editor(&mut self) {
        let Some(editor) = self.snippet_editor.as_ref().cloned() else {
            return;
        };
        let trim_whitespace = self.snippet_settings.filters.trim_whitespace;
        let max_len = self.snippet_settings.advanced.max_clipboard_size;
        let mut title = editor.title;
        let mut content = editor.content;
        if trim_whitespace {
            title = title.trim().to_string();
            content = content.trim().to_string();
        }
        if content.chars().count() > max_len {
            content = content.chars().take(max_len).collect();
        }
        let group_name = self
            .snippet_palette
            .group_name_at(editor.group_index)
            .unwrap_or("General")
            .to_string();
        if self.snippet_settings.filters.ignore_duplicates {
            let duplicate = self
                .snippet_palette
                .store()
                .items()
                .iter()
                .enumerate()
                .find(|(index, snippet)| {
                    let same_entry = match editor.mode {
                        SnippetEditorMode::Edit(current) => *index == current,
                        SnippetEditorMode::Add => false,
                    };
                    !same_entry
                        && snippet.title == title
                        && snippet.content == content
                        && snippet.group.eq_ignore_ascii_case(&group_name)
                })
                .is_some();
            if duplicate {
                return;
            }
        }
        let selected_store_index = match editor.mode {
            SnippetEditorMode::Add => {
                self.snippet_palette
                    .add_snippet(title, content, Some(&group_name))
            }
            SnippetEditorMode::Edit(store_index) => self
                .snippet_palette
                .update_snippet(store_index, title, content, Some(&group_name))
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut clear_ime = false;
        let Some(editor) = self.snippet_group_editor.as_mut() else {
            return false;
        };

        if should_defer_snippet_keystroke_to_ime(&event.keystroke) {
            return true;
        }

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
                    clear_ime = true;
                    self.apply_group_editor_edit();
                    cx.notify();
                }
            }
            "backspace" => {
                if editor.edit_mode != SnippetGroupEditMode::None {
                    clear_ime = true;
                    editor.draft_name.pop();
                    cx.notify();
                }
            }
            _ => {
                if editor.edit_mode == SnippetGroupEditMode::None {
                    return false;
                }
                if let Some(text) = direct_text_from_snippet_keystroke(&event.keystroke) {
                    editor.draft_name.push_str(&text);
                    cx.notify();
                } else if event.keystroke.key_char.is_none() {
                    return false;
                }
            }
        }
        if clear_ime {
            self.clear_snippet_ime_state();
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
                    self.open_snippet_editor(SnippetEditorMode::Edit(store_index), None, window);
                    cx.notify();
                }
            }
            _ => {
                if event.keystroke.modifiers.control && event.keystroke.key == "n" {
                    self.open_snippet_editor(
                        SnippetEditorMode::Add,
                        Some(state.selected_group_index),
                        window,
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

    fn handle_snippet_editor_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut clear_ime = false;
        if self.snippet_editor.is_none() {
            return false;
        }
        if should_defer_snippet_keystroke_to_ime(&event.keystroke) {
            return true;
        }

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.close_snippet_editor();
                cx.notify();
            }
            "v" if event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                clear_ime = true;
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return true;
                };
                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                if paste_text_to_snippet_editor_field(editor, &text) {
                    cx.notify();
                }
            }
            "insert"
                if event.keystroke.modifiers.shift
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
            {
                clear_ime = true;
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return true;
                };
                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                if paste_text_to_snippet_editor_field(editor, &text) {
                    cx.notify();
                }
            }
            "left" => {
                clear_ime = true;
                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                if move_snippet_editor_cursor(editor, TextCursorMove::Left) {
                    cx.notify();
                }
            }
            "right" => {
                clear_ime = true;
                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                if move_snippet_editor_cursor(editor, TextCursorMove::Right) {
                    cx.notify();
                }
            }
            "home" => {
                clear_ime = true;
                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                if move_snippet_editor_cursor(editor, TextCursorMove::Home) {
                    cx.notify();
                }
            }
            "end" => {
                clear_ime = true;
                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                if move_snippet_editor_cursor(editor, TextCursorMove::End) {
                    cx.notify();
                }
            }
            "tab" => {
                clear_ime = true;
                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                editor.active_field = match editor.active_field {
                    SnippetEditorField::Title => SnippetEditorField::Content,
                    SnippetEditorField::Content => SnippetEditorField::Title,
                };
                cx.notify();
            }
            "enter" if event.keystroke.modifiers.control => {
                clear_ime = true;
                self.save_snippet_editor();
                cx.notify();
            }
            "enter" => {
                clear_ime = true;
                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                match editor.active_field {
                    SnippetEditorField::Title => editor.active_field = SnippetEditorField::Content,
                    SnippetEditorField::Content => {
                        insert_text_at_cursor(
                            &mut editor.content,
                            &mut editor.content_cursor,
                            "\n",
                        );
                    }
                }
                cx.notify();
            }
            "backspace" => {
                clear_ime = true;
                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                if delete_text_before_snippet_editor_cursor(editor) {
                    cx.notify();
                }
            }
            _ => {
                let Some(text) = direct_text_from_snippet_keystroke(&event.keystroke) else {
                    return false;
                };

                let Some(editor) = self.snippet_editor.as_mut() else {
                    return false;
                };
                if append_text_to_snippet_editor_field(editor, &text) {
                    cx.notify();
                    return true;
                }
                return false;
            }
        }

        if clear_ime {
            self.clear_snippet_ime_state();
        }
        true
    }

    fn handle_snippet_settings_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.show_snippet_settings {
            return false;
        }

        if should_defer_snippet_keystroke_to_ime(&event.keystroke) {
            return true;
        }

        if event.keystroke.modifiers.control && event.keystroke.key == "s" {
            self.close_snippet_settings(true, cx);
            cx.notify();
            return true;
        }

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.close_snippet_settings(false, cx);
                cx.notify();
                return true;
            }
            "enter" => {
                if self.snippet_settings_active_text
                    == Some(SnippetSettingsTextField::FilterPatternInput)
                {
                    self.finalize_snippet_pattern_input(cx);
                    cx.notify();
                    return true;
                }
                return false;
            }
            "backspace" => {
                if let Some(draft) = self.snippet_settings_draft.as_mut() {
                    match self.snippet_settings_active_text {
                        Some(SnippetSettingsTextField::ShowWindowHotkey) => {
                            draft.hotkeys.show_window.pop();
                            cx.notify();
                            return true;
                        }
                        Some(SnippetSettingsTextField::PasteAsPlainTextHotkey) => {
                            draft.hotkeys.paste_as_plain_text.pop();
                            cx.notify();
                            return true;
                        }
                        Some(SnippetSettingsTextField::QuickPasteHotkey) => {
                            draft.hotkeys.quick_paste.pop();
                            cx.notify();
                            return true;
                        }
                        Some(SnippetSettingsTextField::ShowFavoritesHotkey) => {
                            draft.hotkeys.show_favorites.pop();
                            cx.notify();
                            return true;
                        }
                        Some(SnippetSettingsTextField::ShowTemplatesHotkey) => {
                            draft.hotkeys.show_templates.pop();
                            cx.notify();
                            return true;
                        }
                        Some(SnippetSettingsTextField::FilterPatternInput) => {
                            if let Some(pattern) = draft.filters.exclude_patterns.last_mut() {
                                pattern.pop();
                                cx.notify();
                                return true;
                            }
                        }
                        Some(SnippetSettingsTextField::ExcludeApps) => {
                            draft.advanced.exclude_apps.pop();
                            cx.notify();
                            return true;
                        }
                        None => {}
                    }
                }
            }
            _ => {}
        }

        if let Some(text) = direct_text_from_snippet_keystroke(&event.keystroke) {
            if let Some(draft) = self.snippet_settings_draft.as_mut() {
                match self.snippet_settings_active_text {
                    Some(SnippetSettingsTextField::ShowWindowHotkey) => {
                        draft.hotkeys.show_window.push_str(&text);
                    }
                    Some(SnippetSettingsTextField::PasteAsPlainTextHotkey) => {
                        draft.hotkeys.paste_as_plain_text.push_str(&text);
                    }
                    Some(SnippetSettingsTextField::QuickPasteHotkey) => {
                        draft.hotkeys.quick_paste.push_str(&text);
                    }
                    Some(SnippetSettingsTextField::ShowFavoritesHotkey) => {
                        draft.hotkeys.show_favorites.push_str(&text);
                    }
                    Some(SnippetSettingsTextField::ShowTemplatesHotkey) => {
                        draft.hotkeys.show_templates.push_str(&text);
                    }
                    Some(SnippetSettingsTextField::FilterPatternInput) => {
                        if let Some(pattern) = draft.filters.exclude_patterns.last_mut() {
                            pattern.push_str(&text);
                        }
                    }
                    Some(SnippetSettingsTextField::ExcludeApps) => {
                        draft.advanced.exclude_apps.push_str(&text);
                    }
                    None => return false,
                }
                cx.notify();
                return true;
            }
        }

        false
    }

    fn handle_ai_settings_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.show_settings || self.settings_category != SettingsCategory::Privacy {
            return false;
        }

        let Some(active_field) = self.ai_settings_active_text else {
            return false;
        };

        match event.keystroke.key.as_ref() {
            "escape" | "enter" => {
                self.ai_settings_active_text = None;
                cx.notify();
                return true;
            }
            "backspace" => {
                self.pop_ai_settings_text(active_field, cx);
                return true;
            }
            "v" if event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return true;
                };
                self.append_ai_settings_text(active_field, &text, cx);
                return true;
            }
            "insert"
                if event.keystroke.modifiers.shift
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
            {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return true;
                };
                self.append_ai_settings_text(active_field, &text, cx);
                return true;
            }
            _ => {}
        }

        if let Some(text) = direct_text_from_snippet_keystroke(&event.keystroke) {
            self.append_ai_settings_text(active_field, &text, cx);
            return true;
        }

        false
    }

    fn handle_keyboard_settings_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.show_settings || self.settings_category != SettingsCategory::Keyboard {
            return false;
        }

        let Some(active_field) = self.keyboard_settings_active_text else {
            return false;
        };

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.set_keyboard_shortcut_capture(None, cx);
                return true;
            }
            "backspace" | "delete" => {
                self.set_keyboard_shortcut(active_field, String::new(), cx);
                return true;
            }
            _ => {}
        }

        if let Some(shortcut) = hotkey_string_for_keystroke(&event.keystroke) {
            self.set_keyboard_shortcut(active_field, shortcut, cx);
        }

        true
    }

    fn handle_custom_snippet_hotkeys(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.show_snippet_settings
            || self.show_settings
            || self.snippet_group_editor.is_some()
            || self.snippet_list_editor.is_some()
            || self.snippet_editor.is_some()
            || self.snippet_csv_dialog.is_some()
        {
            return false;
        }

        let hotkeys = &self.snippet_settings.hotkeys;
        if hotkey_matches(event, &hotkeys.show_templates) {
            self.show_shell_menu = false;
            self.show_settings = false;
            self.show_snippet_context_menu = false;
            self.snippet_palette.set_favorites_only(false);
            self.snippet_palette.show();
            window.focus(&self.focus_handle);
            cx.notify();
            return true;
        }

        if hotkey_matches(event, &hotkeys.show_favorites) {
            self.show_shell_menu = false;
            self.show_settings = false;
            self.show_snippet_context_menu = false;
            self.snippet_palette.set_favorites_only(true);
            self.snippet_palette.show();
            window.focus(&self.focus_handle);
            cx.notify();
            return true;
        }

        if hotkey_matches(event, &hotkeys.quick_paste) {
            self.snippet_palette.set_favorites_only(true);
            self.snippet_palette.show();
            self.activate_selected_snippet(window, cx);
            return true;
        }

        if hotkey_matches(event, &hotkeys.paste_as_plain_text) {
            if self.snippet_palette.is_visible() {
                self.activate_selected_snippet(window, cx);
                return true;
            }
        }

        if hotkey_matches(event, &hotkeys.show_window) {
            window.focus(&self.focus_handle);
            cx.notify();
            return true;
        }

        false
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let snippet_ime_target = self.compute_snippet_ime_target();

        if self.handle_snippet_settings_key(event, window, cx)
            || self.handle_ai_settings_key(event, window, cx)
            || self.handle_keyboard_settings_key(event, window, cx)
            || self.handle_snippet_editor_key(event, window, cx)
            || self.handle_snippet_group_editor_key(event, window, cx)
            || self.handle_snippet_list_editor_key(event, window, cx)
        {
            cx.stop_propagation();
            return;
        }

        if snippet_ime_target.is_some()
            && snippet_ime_target != Some(SnippetImeTarget::PaletteQuery)
        {
            cx.stop_propagation();
            return;
        }

        if self.handle_custom_snippet_hotkeys(event, window, cx) {
            cx.stop_propagation();
            return;
        }

        if self.handle_global_shortcut_key(event, window, cx) {
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

        if snippet_ime_target != Some(SnippetImeTarget::PaletteQuery) {
            return;
        }

        if should_defer_snippet_keystroke_to_ime(&event.keystroke) {
            cx.stop_propagation();
            return;
        }

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.snippet_palette.hide();
                self.focus_active_terminal(window, cx);
                cx.notify();
            }
            "enter" => {
                if self.snippet_palette.filtered_entries().is_empty() {
                    if let Some(suggestion) =
                        self.ai_suggestion_state.suggestions().first().cloned()
                    {
                        self.activate_ai_suggestion(suggestion, window, cx);
                    } else {
                        cx.notify();
                    }
                } else {
                    self.activate_selected_snippet(window, cx);
                }
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
                self.clear_snippet_ime_state();
                let mut query = self.snippet_palette.query().to_string();
                if query.pop().is_some() {
                    self.update_palette_query_and_ai(query, cx);
                } else {
                    self.clear_palette_query_and_ai(cx);
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

                if let Some(text) = direct_text_from_snippet_keystroke(&event.keystroke) {
                    let mut query = self.snippet_palette.query().to_string();
                    query.push_str(&text);
                    self.update_palette_query_and_ai(query, cx);
                    cx.notify();
                } else {
                    cx.stop_propagation();
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
                    .window_control_area(WindowControlArea::Close)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.request_window_close(window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .id("traffic-minimize")
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(rgb(YELLOW))
                    .window_control_area(WindowControlArea::Min)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.minimize_window(window);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .id("traffic-maximize")
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(rgb(GREEN))
                    .window_control_area(WindowControlArea::Max)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.toggle_zoom_window(window);
                            cx.stop_propagation();
                        }),
                    ),
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
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.show_shell_menu = false;
                            this.state.update(cx, |state, cx| {
                                state.add_tab_with_shell(&command, cx);
                                cx.notify();
                            });
                            cx.defer_in(window, |this, window, cx| {
                                this.focus_active_terminal(window, cx);
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
        for (category, label, icon_path) in SettingsCategory::all() {
            let active = *category == self.settings_category;
            let category_value = *category;
            let icon_color = if active { rgb(0xffffff) } else { rgb(SUBTEXT1) };
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
                        this.set_keyboard_shortcut_capture(None, cx);
                        this.ai_settings_active_text = None;
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
                    .child(
                        div()
                            .w(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path(*icon_path)
                                    .size(px(15.0))
                                    .text_color(icon_color)
                                    .flex_shrink_0(),
                            ),
                    )
                    .child(*label)
                    .into_any_element(),
            );
        }

        Some(
            div()
                .id("settings-panel")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(Self::on_key_down))
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.close_settings_panel(cx);
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
                                .id("settings-content-scroll")
                                .flex_1()
                                .p(px(24.0))
                                .overflow_scroll()
                                .scrollbar_width(px(6.0))
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
                                if this.state.read(cx).config.clear_history_on_exit {
                                    this.clear_runtime_history(cx);
                                }
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

        let ime_entity = cx.entity();
        let panel_w = (viewport_w - 48.0).clamp(640.0, 860.0);
        let panel_h = (viewport_h - 64.0).clamp(420.0, 600.0);
        let display_settings = self.snippet_settings.display.clone();
        let (top, left) = snippet_modal_origin(
            &display_settings.window_position,
            viewport_w,
            viewport_h,
            panel_w,
            panel_h,
        );
        let query_is_empty = self.snippet_palette.query().is_empty();
        let query_len = self.snippet_palette.query().trim().chars().count();
        let display_text = if query_is_empty {
            "検索...".to_string()
        } else {
            self.snippet_palette.query().to_string()
        };
        let app_config = self.state.read(cx).config.clone();
        let ai_provider = sanitize_ai_provider(&app_config.ai_provider);
        let ai_model_label = resolve_ai_model(ai_provider, &app_config.ai_model);
        let ai_suggestions = self.ai_suggestion_state.suggestions().to_vec();
        let ai_status_message = self.ai_suggestion_status_message(ai_provider);
        let show_ai_section = query_len >= QUERY_MIN_CHARS || ai_status_message.is_some();
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
                .bg(rgba(rgba_with_alpha(0xFFFFFF, display_settings.transparency)))
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
                                .child(
                                    div()
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .rounded_full()
                                        .bg(rgb(RED))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                |this, _: &MouseDownEvent, window, cx| {
                                                    this.snippet_palette.hide();
                                                    this.focus_active_terminal(window, cx);
                                                    cx.notify();
                                                },
                                            ),
                                        ),
                                )
                                .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(YELLOW)))
                                .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(GREEN))),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(12.0))
                                .text_color(rgba(0x6B7280FF))
                                .child("定型文"),
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
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .id("snippet-add-group-btn")
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
                                                .text_color(rgba(0x2F80EDFF))
                                                .hover(|style| style.bg(rgba(0xE5E7EBFF)))
                                                .child(
                                                    div()
                                                        .text_size(px(14.0))
                                                        .font_weight(FontWeight::BOLD)
                                                        .child("+"),
                                                )
                                                .child("グループ追加")
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                                        this.open_new_snippet_group_editor(window);
                                                        cx.notify();
                                                    }),
                                                ),
                                        )
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
                                                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                                        this.open_snippet_settings(window, cx);
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
                                                .relative()
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
                                        )
                                .when(
                                            self.snippet_ime_target
                                                == Some(SnippetImeTarget::PaletteQuery),
                                            |node| {
                                                node.child(snippet_ime_input_overlay(
                                                    ime_entity.clone(),
                                                    self.focus_handle.clone(),
                                                ))
                                            },
                                        ),
                                )
                                .when(show_ai_section, |node| {
                                    node.child(
                                        div()
                                            .w_full()
                                            .px(px(10.0))
                                            .pt(px(10.0))
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .child(
                                                        div()
                                                            .font_family(UI_FONT)
                                                            .text_size(px(11.0))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(rgba(0x6B7280FF))
                                                            .child(format!(
                                                                "{} AI",
                                                                ai_provider.label()
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .font_family(UI_FONT)
                                                            .text_size(px(10.0))
                                                            .text_color(rgba(0x9CA3AFFF))
                                                            .child(ai_model_label.clone()),
                                                    ),
                                            )
                                            .children(ai_status_message.clone().into_iter().map(|message| {
                                                div()
                                                    .w_full()
                                                    .p(px(10.0))
                                                    .rounded(px(10.0))
                                                    .border_1()
                                                    .border_color(rgba(0xBFDBFEFF))
                                                    .bg(rgba(0xEFF6FFFF))
                                                    .font_family(UI_FONT)
                                                    .text_size(px(11.0))
                                                    .text_color(rgba(0x1D4ED8FF))
                                                    .child(message)
                                                    .into_any_element()
                                            }))
                                            .children(ai_suggestions.iter().map(|suggestion| {
                                                let suggestion = suggestion.clone();
                                                let suggestion_for_click = suggestion.clone();
                                                let command_preview = suggestion.command.clone();
                                                div()
                                                    .w_full()
                                                    .p(px(12.0))
                                                    .rounded(px(10.0))
                                                    .border_1()
                                                    .border_color(rgba(0xD6E4FFFF))
                                                    .bg(rgba(0xF8FBFFFF))
                                                    .cursor_pointer()
                                                    .hover(|style| style.border_color(rgba(0x8BB6FFFF)))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                                            this.activate_ai_suggestion(
                                                                suggestion_for_click.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                            cx.stop_propagation();
                                                        }),
                                                    )
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
                                                                    .child(
                                                                        div()
                                                                            .font_family(UI_FONT)
                                                                            .text_size(px(12.0))
                                                                            .font_weight(FontWeight::MEDIUM)
                                                                            .text_color(rgba(0x1F2937FF))
                                                                            .child(suggestion.title),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .font_family(UI_FONT)
                                                                            .text_size(px(11.0))
                                                                            .text_color(rgba(0x6B7280FF))
                                                                            .child(suggestion.detail),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .font_family(MONO_FONT)
                                                                            .text_size(px(11.0))
                                                                            .text_color(rgba(0x1D4ED8FF))
                                                                            .child(command_preview),
                                                                    ),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_size(px(12.0))
                                                                    .text_color(rgba(0x9CA3AFFF))
                                                                    .child("送信"),
                                                            ),
                                                    )
                                                    .into_any_element()
                                            })),
                                    )
                                })
                                .child(
                                    div()
                                        .id("snippet-list-scroll")
                                        .flex_1()
                                        .w_full()
                                        .overflow_scroll()
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
                                                                              window,
                                                                              cx| {
                                                                            this.activate_snippet_store_index(
                                                                                index,
                                                                                window,
                                                                                cx,
                                                                            );
                                                                        },
                                                                    ),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .font_family(UI_FONT)
                                                                        .text_size(px(display_settings.font_size as f32 - 2.0))
                                                                        .font_weight(FontWeight::MEDIUM)
                                                                        .text_color(rgba(0x374151FF))
                                                                        .child(title.clone()),
                                                                )
                                                                .when(display_settings.show_preview, |node| {
                                                                    node.child(
                                                                        div()
                                                                            .font_family(UI_FONT)
                                                                            .text_size(px(display_settings.font_size as f32 - 3.0))
                                                                            .text_color(rgba(0x9CA3AFFF))
                                                                            .child(summary),
                                                                    )
                                                                })
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
                                                                                  window,
                                                                                  cx| {
                                                                                this.open_snippet_editor(
                                                                                    SnippetEditorMode::Edit(index),
                                                                                    None,
                                                                                    window,
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
                                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                        let group_hint = this
                                            .snippet_palette
                                            .active_group()
                                            .and_then(|group| this.snippet_palette.group_index_by_name(group));
                                        this.open_snippet_editor(
                                            SnippetEditorMode::Add,
                                            group_hint,
                                            window,
                                        );
                                        cx.notify();
                                    }),
                                ),
                        ),
                ));
        Some(panel.into_any_element())
    }

    fn render_snippet_settings_panel(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_snippet_settings {
            return None;
        }

        let Some(settings) = self.snippet_settings_draft.clone() else {
            return None;
        };

        let panel_w = 780.0;
        let panel_h = 560.0;
        let (top, left) = snippet_modal_origin(
            &settings.display.window_position,
            viewport_w,
            viewport_h,
            panel_w,
            panel_h,
        );

        let categories = SnippetSettingsCategory::all()
            .iter()
            .copied()
            .map(|category| {
                light_settings_tab(
                    category.label(),
                    category == self.snippet_settings_category,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        this.snippet_settings_category = category;
                        this.snippet_settings_active_text = None;
                        cx.notify();
                    }),
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();

        Some(
            div()
                .id("snippet-settings-panel")
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.close_snippet_settings(false, cx);
                    cx.notify();
                }))
                .absolute()
                .top(px(top))
                .left(px(left))
                .w(px(panel_w))
                .h(px(panel_h))
                .rounded(px(14.0))
                .overflow_hidden()
                .border_1()
                .border_color(rgba(0xD9DADDFF))
                .bg(rgb(0xFFFFFF))
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(28.0))
                        .px(px(14.0))
                        .bg(rgb(0xF4F4F5))
                        .border_b_1()
                        .border_color(rgba(0xE5E7EBFF))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .rounded_full()
                                        .bg(rgb(RED))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                                this.close_snippet_settings(false, cx);
                                                this.focus_active_terminal(window, cx);
                                                cx.notify();
                                            }),
                                        ),
                                )
                                .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(YELLOW)))
                                .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(GREEN))),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(11.0))
                                .text_color(rgba(0x6B7280FF))
                                .child("設定"),
                        )
                        .child(div().w(px(46.0))),
                )
                .child(
                    div()
                        .h(px(40.0))
                        .px(px(16.0))
                        .border_b_1()
                        .border_color(rgba(0xE5E7EBFF))
                        .bg(rgb(0xFAFAFB))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|style| style.bg(rgba(0xEEF2F7FF)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                        this.close_snippet_settings(false, cx);
                                        cx.notify();
                                    }),
                                )
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(14.0))
                                        .text_color(rgba(0x4B5563FF))
                                        .child("<"),
                                ),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(16.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgba(0x1F2937FF))
                                .child("設定"),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .overflow_hidden()
                        .child(
                            div()
                                .w(px(160.0))
                                .h_full()
                                .bg(rgb(0xF6F7F8))
                                .border_r_1()
                                .border_color(rgba(0xE5E7EBFF))
                                .p(px(8.0))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .children(categories),
                        )
                        .child(
                            div()
                                .id("snippet-settings-content-scroll")
                                .flex_1()
                                .h_full()
                                .bg(rgb(0xFFFFFF))
                                .overflow_scroll()
                                .p(px(20.0))
                                .children(Some(self.render_snippet_settings_content(cx))),
                        ),
                )
                .child(
                    div()
                        .h(px(44.0))
                        .px(px(14.0))
                        .border_t_1()
                        .border_color(rgba(0xE5E7EBFF))
                        .bg(rgb(0xFAFAFB))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(10.0))
                                .text_color(rgba(0x9CA3AFFF))
                                .child(format!("ZWG snippets v{}", env!("CARGO_PKG_VERSION"))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(light_secondary_button(
                                    "キャンセル",
                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                        this.close_snippet_settings(false, cx);
                                        cx.notify();
                                    }),
                                ))
                                .child(light_primary_button(
                                    "保存",
                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                        this.close_snippet_settings(true, cx);
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_snippet_settings_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.snippet_settings_category {
            SnippetSettingsCategory::General => {
                self.render_snippet_settings_general(cx).into_any_element()
            }
            SnippetSettingsCategory::Hotkeys => {
                self.render_snippet_settings_hotkeys(cx).into_any_element()
            }
            SnippetSettingsCategory::Display => {
                self.render_snippet_settings_display(cx).into_any_element()
            }
            SnippetSettingsCategory::Filters => {
                self.render_snippet_settings_filters(cx).into_any_element()
            }
            SnippetSettingsCategory::Templates => self
                .render_snippet_settings_templates(cx)
                .into_any_element(),
            SnippetSettingsCategory::Notifications => self
                .render_snippet_settings_notifications(cx)
                .into_any_element(),
            SnippetSettingsCategory::Data => {
                self.render_snippet_settings_data(cx).into_any_element()
            }
            SnippetSettingsCategory::Advanced => {
                self.render_snippet_settings_advanced(cx).into_any_element()
            }
        }
    }

    fn handle_global_shortcut_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let config = self.state.read(cx).config.clone();
        let Some(action) = configured_global_shortcut_action(event, &config) else {
            return false;
        };

        match action {
            GlobalShortcutAction::NewTab => self.on_new_tab(&NewTab, window, cx),
            GlobalShortcutAction::CloseTab => self.on_close_tab(&CloseTab, window, cx),
            GlobalShortcutAction::SplitRight => self.on_split_right(&SplitRight, window, cx),
            GlobalShortcutAction::SplitDown => self.on_split_down(&SplitDown, window, cx),
            GlobalShortcutAction::ClosePane => self.on_close_pane(&ClosePane, window, cx),
            GlobalShortcutAction::FocusNext => self.on_focus_next(&FocusNext, window, cx),
            GlobalShortcutAction::FocusPrev => self.on_focus_prev(&FocusPrev, window, cx),
            GlobalShortcutAction::OpenSettings => self.on_open_settings(&OpenSettings, window, cx),
            GlobalShortcutAction::ToggleSnippetPalette => {
                self.on_toggle_snippet_palette(&ToggleSnippetPalette, window, cx)
            }
            GlobalShortcutAction::SnippetQueuePaste => {
                self.on_snippet_queue_paste(&SnippetQueuePaste, window, cx)
            }
            GlobalShortcutAction::Quit => self.on_quit_requested(&crate::Quit, window, cx),
        }

        true
    }

    fn render_snippet_settings_general(&mut self, cx: &mut Context<Self>) -> Div {
        let settings = self
            .snippet_settings_draft
            .clone()
            .unwrap_or_else(|| self.snippet_settings.clone());
        let general = settings.general;

        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(light_section_title("一般設定"))
            .child(light_toggle_row(
                "起動時に自動起動",
                "Windows 起動時に ZWG Terminal を自動起動します",
                general.launch_on_startup,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.general.launch_on_startup = !draft.general.launch_on_startup;
                    });
                }),
            ))
            .child(light_toggle_row(
                "メニューバーに表示",
                "タイトルバーの定型文ボタンを表示します",
                general.show_in_taskbar,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.general.show_in_taskbar = !draft.general.show_in_taskbar;
                    });
                }),
            ))
            .child(light_toggle_row(
                "トレイに最小化",
                "ウィンドウを閉じたときにトレイ運用を優先します",
                general.minimize_to_tray,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.general.minimize_to_tray = !draft.general.minimize_to_tray;
                    });
                }),
            ))
            .child(light_toggle_row(
                "サウンドを再生",
                "定型文を送信したときにシステムサウンドを鳴らします",
                general.play_sound,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.general.play_sound = !draft.general.play_sound;
                    });
                }),
            ))
            .child(light_toggle_row(
                "自動更新チェック",
                "更新確認フラグを保存します",
                general.check_updates,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.general.check_updates = !draft.general.check_updates;
                    });
                }),
            ))
            .child(light_slider_row(
                "最大履歴数",
                format!(
                    "保存する定型文の上限として扱います: {}",
                    general.max_history_items
                ),
                general.max_history_items.to_string(),
                general.max_history_items as f32 / 5000.0,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.general.max_history_items =
                            draft.general.max_history_items.saturating_sub(100).max(100);
                    });
                }),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.general.max_history_items =
                            (draft.general.max_history_items + 100).min(5000);
                    });
                }),
            ))
    }

    fn render_snippet_settings_hotkeys(&mut self, cx: &mut Context<Self>) -> Div {
        let settings = self
            .snippet_settings_draft
            .clone()
            .unwrap_or_else(|| self.snippet_settings.clone());
        let hotkeys = settings.hotkeys;

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(light_section_title("ホットキー設定"))
            .child(light_editable_row(
                "ウィンドウを表示",
                "現在のウィンドウを前面フォーカスします",
                hotkeys.show_window,
                self.snippet_settings_active_text == Some(SnippetSettingsTextField::ShowWindowHotkey),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.snippet_settings_active_text = Some(SnippetSettingsTextField::ShowWindowHotkey);
                    cx.notify();
                }),
            ))
            .child(light_editable_row(
                "プレーンテキストとして貼り付け",
                "パレット表示中の選択定型文を送信します",
                hotkeys.paste_as_plain_text,
                self.snippet_settings_active_text
                    == Some(SnippetSettingsTextField::PasteAsPlainTextHotkey),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.snippet_settings_active_text =
                        Some(SnippetSettingsTextField::PasteAsPlainTextHotkey);
                    cx.notify();
                }),
            ))
            .child(light_editable_row(
                "クイック貼り付け",
                "お気に入りの先頭項目を即送信します",
                hotkeys.quick_paste,
                self.snippet_settings_active_text == Some(SnippetSettingsTextField::QuickPasteHotkey),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.snippet_settings_active_text = Some(SnippetSettingsTextField::QuickPasteHotkey);
                    cx.notify();
                }),
            ))
            .child(light_editable_row(
                "お気に入りを表示",
                "お気に入りのみ表示した定型文パレットを開きます",
                hotkeys.show_favorites,
                self.snippet_settings_active_text
                    == Some(SnippetSettingsTextField::ShowFavoritesHotkey),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.snippet_settings_active_text =
                        Some(SnippetSettingsTextField::ShowFavoritesHotkey);
                    cx.notify();
                }),
            ))
            .child(light_editable_row(
                "定型文を表示",
                "通常の定型文パレットを開きます",
                hotkeys.show_templates,
                self.snippet_settings_active_text
                    == Some(SnippetSettingsTextField::ShowTemplatesHotkey),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.snippet_settings_active_text =
                        Some(SnippetSettingsTextField::ShowTemplatesHotkey);
                    cx.notify();
                }),
            ))
            .child(light_info_box(
                "入力欄をクリックしてからキー文字を打ち込めます。保存後に現在のウィンドウ内で有効になります。",
            ))
    }

    fn render_snippet_settings_display(&mut self, cx: &mut Context<Self>) -> Div {
        let settings = self
            .snippet_settings_draft
            .clone()
            .unwrap_or_else(|| self.snippet_settings.clone());
        let display = settings.display;

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(light_section_title("表示設定"))
            .child(light_cycle_row(
                "テーマ",
                "定型文パレットの配色テーマです",
                theme_label(&display.theme),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        Self::cycle_snippet_setting(
                            &mut draft.display.theme,
                            &["light", "dark", "auto"],
                        );
                    });
                }),
            ))
            .child(light_cycle_row(
                "ウィンドウの表示位置",
                "定型文パレットと設定モーダルの初期位置です",
                window_position_label(&display.window_position),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        Self::cycle_snippet_setting(
                            &mut draft.display.window_position,
                            &[
                                "center",
                                "cursor",
                                "topLeft",
                                "topRight",
                                "bottomLeft",
                                "bottomRight",
                            ],
                        );
                    });
                }),
            ))
            .child(light_cycle_row(
                "1ページあたりの表示件数",
                "パレットの可視行数目安として扱います",
                format!("{}件", display.items_per_page),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        let options = [10, 20, 50, 100];
                        let current = options
                            .iter()
                            .position(|value| *value == draft.display.items_per_page)
                            .unwrap_or(1);
                        draft.display.items_per_page = options[(current + 1) % options.len()];
                    });
                }),
            ))
            .child(light_slider_row(
                "フォントサイズ",
                format!("定型文パレットの UI スケール: {}px", display.font_size),
                format!("{}px", display.font_size),
                display.font_size as f32 / 20.0,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.display.font_size = draft.display.font_size.saturating_sub(1).max(10);
                    });
                }),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.display.font_size = (draft.display.font_size + 1).min(20);
                    });
                }),
            ))
            .child(light_slider_row(
                "透明度",
                format!("モーダル背景の不透明度: {}%", display.transparency),
                format!("{}%", display.transparency),
                display.transparency as f32 / 100.0,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.display.transparency =
                            draft.display.transparency.saturating_sub(5).max(50);
                    });
                }),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.display.transparency = (draft.display.transparency + 5).min(100);
                    });
                }),
            ))
            .child(light_toggle_row(
                "プレビューを表示",
                "定型文カードに内容の 1 行目を表示します",
                display.show_preview,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.display.show_preview = !draft.display.show_preview;
                    });
                }),
            ))
    }

    fn render_snippet_settings_filters(&mut self, cx: &mut Context<Self>) -> Div {
        let settings = self
            .snippet_settings_draft
            .clone()
            .unwrap_or_else(|| self.snippet_settings.clone());
        let filters = settings.filters;
        let editing_pattern = if self.snippet_settings_active_text
            == Some(SnippetSettingsTextField::FilterPatternInput)
        {
            filters.exclude_patterns.last().cloned().unwrap_or_default()
        } else {
            String::new()
        };

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(light_section_title("フィルタ設定"))
            .child(light_toggle_row(
                "大文字小文字を無視",
                "検索時にケースを無視します",
                filters.ignore_case,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.filters.ignore_case = !draft.filters.ignore_case;
                    });
                }),
            ))
            .child(light_toggle_row(
                "前後の空白を除去",
                "定型文保存時の余分な空白を抑制します",
                filters.trim_whitespace,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.filters.trim_whitespace = !draft.filters.trim_whitespace;
                    });
                }),
            ))
            .child(light_toggle_row(
                "重複を無視",
                "同一内容の登録抑止フラグを保存します",
                filters.ignore_duplicates,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.filters.ignore_duplicates = !draft.filters.ignore_duplicates;
                    });
                }),
            ))
            .child(light_slider_row(
                "最小文字数",
                format!(
                    "この文字数未満の定型文は一覧から除外します: {}",
                    filters.min_text_length
                ),
                filters.min_text_length.to_string(),
                filters.min_text_length.min(20) as f32 / 20.0,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.filters.min_text_length =
                            draft.filters.min_text_length.saturating_sub(1).max(1);
                    });
                }),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.filters.min_text_length =
                            (draft.filters.min_text_length + 1).min(1000);
                    });
                }),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(light_section_subtitle("除外パターン"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(light_text_input_box(
                                editing_pattern,
                                "例: password",
                                self.snippet_settings_active_text
                                    == Some(SnippetSettingsTextField::FilterPatternInput),
                                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                    this.update_snippet_settings_draft(cx, |draft| {
                                        if draft.filters.exclude_patterns.last().is_none()
                                            || draft
                                                .filters
                                                .exclude_patterns
                                                .last()
                                                .map(|pattern| !pattern.is_empty())
                                                .unwrap_or(false)
                                        {
                                            draft.filters.exclude_patterns.push(String::new());
                                        }
                                    });
                                    this.snippet_settings_active_text =
                                        Some(SnippetSettingsTextField::FilterPatternInput);
                                    cx.notify();
                                }),
                            ))
                            .child(light_square_button(
                                "+",
                                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                    if this.snippet_settings_active_text
                                        == Some(SnippetSettingsTextField::FilterPatternInput)
                                    {
                                        this.finalize_snippet_pattern_input(cx);
                                    } else {
                                        this.add_snippet_exclude_pattern(cx);
                                    }
                                    cx.notify();
                                }),
                            )),
                    )
                    .children(
                        filters
                            .exclude_patterns
                            .iter()
                            .enumerate()
                            .filter(|(_, pattern)| !pattern.trim().is_empty())
                            .map(|(index, pattern)| {
                                light_pattern_chip(
                                    pattern.clone(),
                                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                        this.remove_snippet_exclude_pattern(index, cx);
                                    }),
                                )
                                .into_any_element()
                            })
                            .collect::<Vec<_>>(),
                    ),
            )
    }

    fn render_snippet_settings_templates(&mut self, cx: &mut Context<Self>) -> Div {
        let items = self.snippet_palette.store().items().to_vec();

        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(light_section_title("定型文管理"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(light_outline_button(
                                "グループ新規作成",
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.open_new_snippet_group_editor(window);
                                    cx.notify();
                                }),
                            ))
                            .child(light_outline_button(
                                "定型文新規作成",
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.open_snippet_editor(SnippetEditorMode::Add, None, window);
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .children(if items.is_empty() {
                vec![light_empty_state("定型文が登録されていません").into_any_element()]
            } else {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let preview = item.content.replace('\n', " ");
                        light_template_card(
                            item.title.clone(),
                            item.group.clone(),
                            preview,
                            item.is_favorite,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                this.open_snippet_editor(
                                    SnippetEditorMode::Edit(index),
                                    None,
                                    window,
                                );
                                cx.notify();
                            }),
                            cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                let _ = this.snippet_palette.delete_snippet(index);
                                cx.notify();
                            }),
                        )
                        .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
    }

    fn render_snippet_settings_notifications(&mut self, cx: &mut Context<Self>) -> Div {
        let settings = self
            .snippet_settings_draft
            .clone()
            .unwrap_or_else(|| self.snippet_settings.clone());
        let notifications = settings.notifications;

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(light_section_title("通知設定"))
            .child(light_toggle_row(
                "コピー時に通知",
                "定型文設定の保存や入出力結果を通知します",
                notifications.show_copy_notification,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.notifications.show_copy_notification =
                            !draft.notifications.show_copy_notification;
                    });
                }),
            ))
            .child(light_toggle_row(
                "貼り付け時に通知",
                "定型文送信時にトーストを表示します",
                notifications.show_paste_notification,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.notifications.show_paste_notification =
                            !draft.notifications.show_paste_notification;
                    });
                }),
            ))
            .child(light_cycle_row(
                "通知の表示位置",
                "簡易トーストの表示位置です",
                notification_position_label(&notifications.notification_position),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        Self::cycle_snippet_setting(
                            &mut draft.notifications.notification_position,
                            &["topLeft", "topRight", "bottomLeft", "bottomRight"],
                        );
                    });
                }),
            ))
            .child(light_slider_row(
                "通知の表示時間",
                format!(
                    "{} 秒表示します",
                    notifications.notification_duration as f32 / 1000.0
                ),
                format!("{}ms", notifications.notification_duration),
                notifications.notification_duration as f32 / 5000.0,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.notifications.notification_duration = draft
                            .notifications
                            .notification_duration
                            .saturating_sub(500)
                            .max(1000);
                    });
                }),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.notifications.notification_duration =
                            (draft.notifications.notification_duration + 500).min(5000);
                    });
                }),
            ))
            .child(light_preview_box(
                "通知のプレビュー",
                "定型文を送信しました",
                "echo \"Hello, world!\"",
            ))
    }

    fn render_snippet_settings_data(&mut self, cx: &mut Context<Self>) -> Div {
        let settings_size = std::fs::metadata(crate::snippets::SnippetSettings::default_path())
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let snippets_size = std::fs::metadata(self.snippet_palette.store_path())
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(light_section_title("データ管理"))
            .child(light_action_card(
                "設定のエクスポート",
                "現在の定型文設定を JSON として書き出します",
                "設定をエクスポート",
                false,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.export_snippet_settings_json(cx);
                }),
            ))
            .child(light_action_card(
                "設定のインポート",
                "以前書き出した JSON を読み込みます",
                "設定をインポート",
                false,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.import_snippet_settings_json(cx);
                }),
            ))
            .child(
                div()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(rgba(0xE5E7EBFF))
                    .bg(rgb(0xFFFFFF))
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(light_section_subtitle("バックアップ"))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(light_secondary_button(
                                "バックアップ作成",
                                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                    this.backup_snippets_to_file(cx);
                                }),
                            ))
                            .child(light_secondary_button(
                                "バックアップ復元",
                                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                    this.restore_snippet_backup(cx);
                                }),
                            )),
                    ),
            )
            .child(light_action_card(
                "データの削除",
                "全ての定型文を即時削除します。元に戻せません",
                "全ての定型文を削除",
                true,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.clear_all_snippets(cx);
                }),
            ))
            .child(light_storage_card(
                self.snippet_palette.item_count(),
                self.snippet_palette.favorite_count(),
                self.snippet_palette.queue_len(),
                settings_size + snippets_size,
            ))
    }

    fn render_snippet_settings_advanced(&mut self, cx: &mut Context<Self>) -> Div {
        let settings = self
            .snippet_settings_draft
            .clone()
            .unwrap_or_else(|| self.snippet_settings.clone());
        let advanced = settings.advanced;

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(light_section_title("詳細設定"))
            .child(light_toggle_row(
                "自動保存",
                "設定値の即時保存フラグを保持します",
                advanced.auto_save,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.advanced.auto_save = !draft.advanced.auto_save;
                    });
                }),
            ))
            .child(light_toggle_row(
                "クリップボード監視",
                "将来の連携用フラグを保持します",
                advanced.monitor_clipboard,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.advanced.monitor_clipboard = !draft.advanced.monitor_clipboard;
                    });
                }),
            ))
            .child(light_toggle_row(
                "FIFO モード",
                "保存後に定型文キューを FIFO にします",
                advanced.fifo_mode,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.advanced.fifo_mode = !draft.advanced.fifo_mode;
                    });
                }),
            ))
            .child(light_toggle_row(
                "パスワード保護",
                "保護フラグを保持します",
                advanced.enable_password,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.advanced.enable_password = !draft.advanced.enable_password;
                    });
                }),
            ))
            .child(light_slider_row(
                "最大クリップボードサイズ",
                format!("登録可能な本文上限: {} 文字", advanced.max_clipboard_size),
                advanced.max_clipboard_size.to_string(),
                advanced.max_clipboard_size as f32 / 50_000.0,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.advanced.max_clipboard_size = draft
                            .advanced
                            .max_clipboard_size
                            .saturating_sub(1000)
                            .max(1000);
                    });
                }),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.update_snippet_settings_draft(cx, |draft| {
                        draft.advanced.max_clipboard_size =
                            (draft.advanced.max_clipboard_size + 1000).min(50_000);
                    });
                }),
            ))
            .child(light_editable_multiline_row(
                "除外するアプリケーション",
                "カンマ区切りで管理用メモを保存します",
                advanced.exclude_apps,
                self.snippet_settings_active_text == Some(SnippetSettingsTextField::ExcludeApps),
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.snippet_settings_active_text = Some(SnippetSettingsTextField::ExcludeApps);
                    cx.notify();
                }),
            ))
            .child(light_warning_box(
                "詳細設定の一部は将来のクリップボード機能拡張に向けた保存項目です。",
            ))
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
                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                this.show_snippet_context_menu = false;
                                this.open_snippet_group_editor(window);
                                cx.notify();
                            }),
                        ))
                        .child(snippet_menu_item(
                            "定型文の編集(T)",
                            false,
                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                this.show_snippet_context_menu = false;
                                this.open_snippet_list_editor(window);
                                cx.notify();
                            }),
                        ))
                        .child(snippet_menu_item(
                            "定型文CSV出力/取込(V)",
                            false,
                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                this.show_snippet_context_menu = false;
                                this.open_snippet_csv_dialog(window);
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
        let ime_entity = cx.entity();
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
                            .font_family(MONO_FONT)
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
                                        .font_family(MONO_FONT)
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
                                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
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
                                                    window.focus(&this.focus_handle);
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
                                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                                if let Some(editor) =
                                                    this.snippet_group_editor.as_mut()
                                                {
                                                    let _ = begin_new_snippet_group_edit(
                                                        &mut this.snippet_palette,
                                                        editor,
                                                    );
                                                    window.focus(&this.focus_handle);
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
                                        .relative()
                                        .rounded(px(8.0))
                                        .border_1()
                                        .border_color(rgb(if editing { ACCENT } else { SURFACE1 }))
                                        .bg(rgba(0xffffff08))
                                        .px(px(12.0))
                                        .flex()
                                        .items_center()
                                        .font_family(UI_FONT)
                                        .text_size(px(12.0))
                                        .text_color(rgb(TEXT))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                                if this
                                                    .snippet_group_editor
                                                    .as_ref()
                                                    .map(|editor| {
                                                        editor.edit_mode
                                                            != SnippetGroupEditMode::None
                                                    })
                                                    .unwrap_or(false)
                                                {
                                                    window.focus(&this.focus_handle);
                                                    cx.notify();
                                                }
                                            }),
                                        )
                                        .child(if editing {
                                            editor.draft_name
                                        } else {
                                            String::new()
                                        })
                                        .when(
                                            editing
                                                && self.snippet_ime_target
                                                    == Some(SnippetImeTarget::GroupName),
                                            |node| {
                                                node.child(snippet_ime_input_overlay(
                                                    ime_entity.clone(),
                                                    self.focus_handle.clone(),
                                                ))
                                            },
                                        ),
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
                            .font_family(UI_FONT)
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
                            .font_family(MONO_FONT)
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
                            .font_family(MONO_FONT)
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
                                        .font_family(MONO_FONT)
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
                                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                                let group_hint = this
                                                    .snippet_list_editor
                                                    .as_ref()
                                                    .map(|state| state.selected_group_index);
                                                this.open_snippet_editor(
                                                    SnippetEditorMode::Add,
                                                    group_hint,
                                                    window,
                                                );
                                                cx.notify();
                                            }),
                                        ))
                                        .child(interactive_action_button(
                                            "編集",
                                            false,
                                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                                let selected =
                                                    this.snippet_list_editor.as_ref().and_then(
                                                        |state| state.selected_item_store_index,
                                                    );
                                                if let Some(selected) = selected {
                                                    this.open_snippet_editor(
                                                        SnippetEditorMode::Edit(selected),
                                                        None,
                                                        window,
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
        let ime_entity = cx.entity();
        let snippet_input_focus_handle = self.snippet_input_focus_handle.clone();
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
        let (title_display, content_display) = active_snippet_editor_text_with_cursor(&editor);

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
                        .track_focus(&self.snippet_input_focus_handle)
                        .on_key_down(cx.listener(Self::on_key_down))
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
                                        .font_family(MONO_FONT)
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
                                                .relative()
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
                                                .font_family(UI_FONT)
                                                .text_size(px(12.0))
                                                .text_color(rgb(TEXT))
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(
                                                        |this, _: &MouseDownEvent, window, cx| {
                                                            if let Some(editor) =
                                                                this.snippet_editor.as_mut()
                                                            {
                                                                editor.active_field =
                                                                    SnippetEditorField::Title;
                                                                window.focus(
                                                                    &this
                                                                        .snippet_input_focus_handle,
                                                                );
                                                                cx.notify();
                                                            }
                                                        },
                                                    ),
                                                )
                                                .child(title_display)
                                                .when(title_active, |node| {
                                                    node.child(snippet_ime_input_overlay(
                                                        ime_entity.clone(),
                                                        snippet_input_focus_handle.clone(),
                                                    ))
                                                }),
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
                                                .relative()
                                                .rounded(px(8.0))
                                                .border_1()
                                                .border_color(rgb(if content_active {
                                                    ACCENT
                                                } else {
                                                    SURFACE1
                                                }))
                                                .bg(rgba(0xffffff08))
                                                .p(px(12.0))
                                                .font_family(MONO_FONT)
                                                .text_size(px(12.0))
                                                .text_color(rgb(TEXT))
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(
                                                        |this, _: &MouseDownEvent, window, cx| {
                                                            if let Some(editor) =
                                                                this.snippet_editor.as_mut()
                                                            {
                                                                editor.active_field =
                                                                    SnippetEditorField::Content;
                                                                window.focus(
                                                                    &this
                                                                        .snippet_input_focus_handle,
                                                                );
                                                                cx.notify();
                                                            }
                                                        },
                                                    ),
                                                )
                                                .child(content_display)
                                                .when(content_active, |node| {
                                                    node.child(snippet_ime_input_overlay(
                                                        ime_entity.clone(),
                                                        snippet_input_focus_handle.clone(),
                                                    ))
                                                }),
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
                                        .font_family(MONO_FONT)
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
            SettingsCategory::Appearance => self.render_appearance_settings(cx).into_any_element(),
            SettingsCategory::Profiles => self.render_profiles_settings().into_any_element(),
            SettingsCategory::Keyboard => self.render_keyboard_settings(cx).into_any_element(),
            SettingsCategory::Notifications => {
                self.render_notifications_settings(cx).into_any_element()
            }
            SettingsCategory::Privacy => self.render_privacy_settings(cx).into_any_element(),
            SettingsCategory::Advanced => self.render_advanced_settings(cx).into_any_element(),
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

    fn render_appearance_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let config = self.state.read(cx).config.clone();
        let mut theme_rows_top: Vec<AnyElement> = Vec::new();
        let current_theme = config.theme.clone();
        let background_image_label = config
            .background_image_path
            .as_deref()
            .and_then(|path| std::path::Path::new(path).file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "未選択".to_string());
        let background_image_path = config.background_image_path.clone();
        let background_image_opacity_ratio = config.background_image_opacity as f32 / 100.0;

        for (name, preview) in THEME_PREVIEWS {
            let selected = current_theme == name;
            theme_rows_top.push(
                theme_card(name, preview, selected)
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(0xffffff08)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            this.set_theme(name, cx);
                        }),
                    )
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(24.0))
            .child(settings_section_heading("テーマ"))
            .child(div().flex().gap(px(12.0)).children(theme_rows_top))
            .child(section_divider())
            .child(settings_section_heading("フォント"))
            .child(settings_row(
                "フォントファミリー",
                interactive_select_box(
                    config.font.family.clone(),
                    180.0,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.cycle_font_family(1, cx);
                    }),
                ),
            ))
            .child(settings_row(
                "フォントサイズ",
                number_stepper(
                    format!("{:.0}px", config.font.size),
                    96.0,
                    false,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.adjust_font_size(-1, cx);
                    }),
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.adjust_font_size(1, cx);
                    }),
                ),
            ))
            .child(section_divider())
            .child(settings_section_heading("カーソル"))
            .child(settings_row(
                "カーソル点滅",
                interactive_toggle(
                    config.cursor_blink,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_cursor_blink(cx);
                    }),
                ),
            ))
            .child(section_divider())
            .child(settings_section_heading("背景画像"))
            .child(settings_row(
                "画像ファイル",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(input_box_dynamic(background_image_label, 220.0, false))
                    .child(interactive_action_button(
                        "読み込み",
                        false,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.pick_background_image(cx);
                        }),
                    ))
                    .when(background_image_path.is_some(), |row| {
                        row.child(interactive_action_button(
                            "解除",
                            false,
                            cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                this.clear_background_image(cx);
                            }),
                        ))
                    }),
            ))
            .when_some(background_image_path.clone(), |section, path| {
                section.child(
                    div()
                        .font_family(UI_FONT)
                        .text_size(px(12.0))
                        .text_color(rgb(SUBTEXT1))
                        .child(path),
                )
            })
            .child(settings_row(
                "画像の透明度",
                interactive_slider_with_value(
                    background_image_opacity_ratio,
                    format!("{}%", config.background_image_opacity),
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.adjust_background_image_opacity(-5, cx);
                    }),
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.adjust_background_image_opacity(5, cx);
                    }),
                ),
            ))
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(12.0))
                    .text_color(rgb(SUBTEXT1))
                    .child("テーマ、フォント、背景画像、カーソル点滅は即時保存されます。"),
            )
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

    fn render_keyboard_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let keyboard = self.state.read(cx).config.keyboard.clone();

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("キーボードショートカット"))
            .child(light_editable_row(
                "新しいタブ",
                "新規タブを開きます",
                keyboard.new_tab,
                self.keyboard_settings_active_text == Some(KeyboardSettingsTextField::NewTab),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.set_keyboard_shortcut_capture(
                        Some(KeyboardSettingsTextField::NewTab),
                        cx,
                    );
                    window.focus(&this.focus_handle);
                }),
            ))
            .child(light_editable_row(
                "タブを閉じる",
                "現在のタブを閉じます",
                keyboard.close_tab,
                self.keyboard_settings_active_text == Some(KeyboardSettingsTextField::CloseTab),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.set_keyboard_shortcut_capture(
                        Some(KeyboardSettingsTextField::CloseTab),
                        cx,
                    );
                    window.focus(&this.focus_handle);
                }),
            ))
            .child(light_editable_row(
                "ペインを左右分割",
                "現在のペインを左右に分割します",
                keyboard.split_right,
                self.keyboard_settings_active_text == Some(KeyboardSettingsTextField::SplitRight),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.set_keyboard_shortcut_capture(
                        Some(KeyboardSettingsTextField::SplitRight),
                        cx,
                    );
                    window.focus(&this.focus_handle);
                }),
            ))
            .child(light_editable_row(
                "ペインを上下分割",
                "現在のペインを上下に分割します",
                keyboard.split_down,
                self.keyboard_settings_active_text == Some(KeyboardSettingsTextField::SplitDown),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.set_keyboard_shortcut_capture(
                        Some(KeyboardSettingsTextField::SplitDown),
                        cx,
                    );
                    window.focus(&this.focus_handle);
                }),
            ))
            .child(light_editable_row(
                "ペインを閉じる",
                "現在フォーカス中のペインを閉じます",
                keyboard.close_pane,
                self.keyboard_settings_active_text == Some(KeyboardSettingsTextField::ClosePane),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.set_keyboard_shortcut_capture(
                        Some(KeyboardSettingsTextField::ClosePane),
                        cx,
                    );
                    window.focus(&this.focus_handle);
                }),
            ))
            .child(light_editable_row(
                "次のペインへ移動",
                "次のペインへフォーカスを移します",
                keyboard.focus_next_pane,
                self.keyboard_settings_active_text
                    == Some(KeyboardSettingsTextField::FocusNextPane),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.set_keyboard_shortcut_capture(
                        Some(KeyboardSettingsTextField::FocusNextPane),
                        cx,
                    );
                    window.focus(&this.focus_handle);
                }),
            ))
            .child(light_editable_row(
                "前のペインへ移動",
                "前のペインへフォーカスを移します",
                keyboard.focus_prev_pane,
                self.keyboard_settings_active_text
                    == Some(KeyboardSettingsTextField::FocusPrevPane),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.set_keyboard_shortcut_capture(
                        Some(KeyboardSettingsTextField::FocusPrevPane),
                        cx,
                    );
                    window.focus(&this.focus_handle);
                }),
            ))
            .child(light_editable_row(
                "設定を開く",
                "設定パネルを開きます",
                keyboard.open_settings,
                self.keyboard_settings_active_text
                    == Some(KeyboardSettingsTextField::OpenSettings),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.set_keyboard_shortcut_capture(
                        Some(KeyboardSettingsTextField::OpenSettings),
                        cx,
                    );
                    window.focus(&this.focus_handle);
                }),
            ))
            .child(light_info_box(
                "項目をクリックしてからショートカットを押すと即時保存されます。既定では上下分割は Ctrl+Shift+S です。Esc で編集終了、Backspace/Delete で解除します。",
            ))
    }

    fn render_notifications_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let config = self.state.read(cx).config.clone();

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("通知"))
            .child(settings_row(
                "ベル音",
                interactive_toggle(
                    config.notification_bell,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_notification_bell(cx);
                    }),
                ),
            ))
            .child(settings_row(
                "ビジュアルベル",
                interactive_toggle(
                    config.visual_bell,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_visual_bell(cx);
                    }),
                ),
            ))
            .child(settings_row(
                "処理完了アラート",
                interactive_toggle(
                    config.process_completion_alert,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_process_completion_alert(cx);
                    }),
                ),
            ))
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(12.0))
                    .text_color(rgb(SUBTEXT1))
                    .child("ターミナルのプロセス終了時に視覚通知やベル音を出します。"),
            )
    }

    fn render_privacy_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let config = self.state.read(cx).config.clone();
        let provider = sanitize_ai_provider(&config.ai_provider);
        let resolved_api_key = resolve_ai_api_key(provider, &config.ai_api_key);
        let resolved_model = resolve_ai_model(provider, &config.ai_model);
        let api_key_source = if config.ai_api_key.trim().is_empty() {
            if resolved_api_key.is_some() {
                format!("環境変数 {} を使用", provider.api_env())
            } else {
                "未設定".to_string()
            }
        } else {
            "設定画面の値を使用".to_string()
        };

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("プライバシーとセキュリティ"))
            .child(settings_row(
                "選択時にコピー",
                interactive_toggle(
                    config.copy_on_select,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_copy_on_select(cx);
                    }),
                ),
            ))
            .child(settings_row(
                "終了時に履歴を消去",
                interactive_toggle(
                    config.clear_history_on_exit,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_clear_history_on_exit(cx);
                    }),
                ),
            ))
            .child(settings_row(
                "AI サジェストを有効化",
                interactive_toggle(
                    config.ai_suggestions_enabled,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_ai_suggestions_enabled(cx);
                    }),
                ),
            ))
            .child(section_divider())
            .child(settings_section_heading("AI サジェスト"))
            .child(settings_row(
                "プロバイダ",
                interactive_cycle_box_dynamic(
                    provider.label().to_string(),
                    180.0,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.ai_settings_active_text = None;
                        this.cycle_ai_provider(cx);
                        window.focus(&this.focus_handle);
                    }),
                ),
            ))
            .child(settings_row(
                "API キー",
                interactive_input_box_dynamic(
                    config.ai_api_key.clone(),
                    "未入力時は環境変数を利用",
                    260.0,
                    true,
                    self.ai_settings_active_text == Some(AiSettingsTextField::ApiKey),
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.ai_settings_active_text = Some(AiSettingsTextField::ApiKey);
                        window.focus(&this.focus_handle);
                        cx.notify();
                    }),
                ),
            ))
            .child(settings_row(
                "モデル",
                interactive_input_box_dynamic(
                    config.ai_model.clone(),
                    "空欄で既定モデルを利用",
                    220.0,
                    true,
                    self.ai_settings_active_text == Some(AiSettingsTextField::Model),
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.ai_settings_active_text = Some(AiSettingsTextField::Model);
                        window.focus(&this.focus_handle);
                        cx.notify();
                    }),
                ),
            ))
            .child(
                div()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(rgba(0xBFDBFEFF))
                    .bg(rgba(0xEFF6FFFF))
                    .p(px(12.0))
                    .font_family(UI_FONT)
                    .text_size(px(11.0))
                    .text_color(rgba(0x1D4ED8FF))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(format!(
                        "コマンドパレット入力時に {} API を直接呼び、候補を即時表示します。",
                        provider.label()
                    ))
                    .child(format!(
                        "API キー状態: {}",
                        if resolved_api_key.is_some() {
                            api_key_source
                        } else {
                            format!("{} を設定してください", provider.api_env())
                        }
                    ))
                    .child(format!(
                        "機能状態: {}",
                        if config.ai_suggestions_enabled {
                            "有効"
                        } else {
                            "無効"
                        }
                    ))
                    .child(format!("実行モデル: {resolved_model}")),
            )
    }

    fn render_advanced_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let config = self.state.read(cx).config.clone();
        let gpu_enabled = config.gpu_acceleration || env_gpu_acceleration_enabled();

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings_section_heading("詳細設定"))
            .child(settings_row(
                "GPU アクセラレーション",
                interactive_toggle(
                    gpu_enabled,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_gpu_acceleration(cx);
                    }),
                ),
            ))
            .child(settings_row(
                "ステータスバーを表示",
                interactive_toggle(
                    config.status_bar_visible,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.toggle_status_bar_visibility(cx);
                    }),
                ),
            ))
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
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(0xFF453A26)))
                    .child("すべての設定を初期値に戻す")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.reset_app_config(cx);
                        }),
                    ),
            )
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(12.0))
                    .text_color(rgb(SUBTEXT1))
                    .child(
                        "GPU は設定か ZWG_ENABLE_DX12_GPU 環境変数で有効化されます。変更は即時反映されます。",
                    ),
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
        self.maybe_clear_snippet_notice();
        self.maybe_clear_app_notice();
        self.sync_snippet_ime_target();
        self.process_terminal_notifications(cx);

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
        let show_snippets_button = self.snippet_settings.general.show_in_taskbar;
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
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.state.update(cx, |state, _cx| {
                            state.active_tab = idx;
                        });
                        cx.notify();
                        cx.defer_in(window, |this, window, cx| {
                            this.focus_active_terminal(window, cx);
                        });
                        cx.stop_propagation();
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
                                cx.stop_propagation();
                            }),
                        )
                        .child("x"),
                );
            }

            tab_elements.push(tab.into_any_element());
        }

        let side_cluster_width = titlebar_side_cluster_width(show_snippets_button);

        let mut titlebar_actions = div()
            .id("titlebar-actions")
            .w(px(titlebar_actions_width(show_snippets_button)))
            .min_w(px(titlebar_actions_width(show_snippets_button)))
            .h(px(24.0))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_end()
            .gap(px(4.0))
            .child(chrome_button("title-add", "ui/plus.svg").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.show_settings = false;
                    this.show_snippet_settings = false;
                    this.show_snippet_context_menu = false;
                    this.snippet_palette.hide();
                    this.show_shell_menu = true;
                    cx.notify();
                    cx.stop_propagation();
                }),
            ))
            .child(
                chrome_button("title-shells", "ui/chevron-down.svg").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.show_settings = false;
                        this.show_snippet_settings = false;
                        this.show_snippet_context_menu = false;
                        this.snippet_palette.hide();
                        this.show_shell_menu = true;
                        cx.notify();
                        cx.stop_propagation();
                    }),
                ),
            );

        if show_snippets_button {
            titlebar_actions = titlebar_actions.child(
                chrome_button("title-snippets", "ui/snippets.svg")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.show_shell_menu = false;
                            this.show_settings = false;
                            this.show_snippet_settings = false;
                            this.show_close_confirm = false;
                            this.show_snippet_context_menu = false;
                            this.snippet_palette.set_favorites_only(false);
                            this.snippet_palette.toggle();
                            if this.snippet_palette.is_visible() {
                                window.focus(&this.focus_handle);
                            } else {
                                this.focus_active_terminal(window, cx);
                            }
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.show_shell_menu = false;
                            this.show_settings = false;
                            this.show_snippet_settings = false;
                            this.show_close_confirm = false;
                            this.snippet_palette.hide();
                            this.show_snippet_context_menu = !this.show_snippet_context_menu;
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    ),
            );
        }

        titlebar_actions = titlebar_actions.child(
            chrome_button("title-settings", "ui/settings.svg").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.show_snippet_context_menu = false;
                    this.snippet_palette.hide();
                    this.open_settings_panel(window, cx);
                    cx.stop_propagation();
                }),
            ),
        );

        let chrome_radius = if new_state.maximized {
            px(0.0)
        } else {
            px(WINDOW_CHROME_RADIUS)
        };

        let title_bar = div()
            .id("title-bar")
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_event, window, _cx| {
                start_titlebar_drag(window);
            })
            .relative()
            .h(px(38.0))
            .w_full()
            .px(px(12.0))
            .border_b_1()
            .border_color(rgba(0xffffff08))
            .bg(linear_gradient(
                180.0,
                linear_color_stop(rgba(0x4a4a4cd8), 0.0),
                linear_color_stop(rgba(0x2f2f31f0), 1.0),
            ))
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(side_cluster_width))
                    .min_w(px(side_cluster_width))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .child(self.render_window_traffic_lights(window, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .mx(px(16.0))
                    .overflow_hidden()
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
                    })
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag)
                            .on_mouse_down(MouseButton::Left, |_event, window, _cx| {
                                start_titlebar_drag(window);
                            }),
                    ),
            )
            .child(
                div()
                    .w(px(side_cluster_width))
                    .min_w(px(side_cluster_width))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_end()
                    .child(titlebar_actions),
            );

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

        let snippet_settings_backdrop = if self.show_snippet_settings {
            Some(
                div()
                    .id("snippet-settings-backdrop")
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

        self.sync_terminal_input_suppression(cx);

        let window_surface = div()
            .id("window-surface")
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(WINDOW_BG))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(1.0))
                    .bg(rgba(0xffffff18)),
            )
            .child(title_bar)
            .child(
                div()
                    .id("terminal-area")
                    .flex_1()
                    .overflow_hidden()
                    .bg(rgb(WINDOW_BG))
                    .children(active_split),
            )
            .when(self.state.read(cx).config.status_bar_visible, |root| {
                root.child(Self::render_status_bar(
                    &active_shell_name,
                    pane_count,
                    tab_count,
                    &grid_label,
                ))
            })
            .children(shell_backdrop)
            .children(self.render_shell_selector(
                new_state.width,
                new_state.height,
                &available_shells,
                cx,
            ))
            .children(settings_backdrop)
            .children(self.render_settings_panel(new_state.width, new_state.height, window, cx))
            .children(snippet_settings_backdrop)
            .children(self.render_snippet_settings_panel(
                new_state.width,
                new_state.height,
                window,
                cx,
            ))
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
            .children(render_snippet_notice(
                self.snippet_notice.as_ref(),
                &self.snippet_settings.notifications.notification_position,
            ))
            .children(render_app_notice(self.app_notice.as_ref()))
            .children(close_confirm_backdrop)
            .children(self.render_close_confirm_dialog(new_state.width, new_state.height, cx));

        div()
            .id("root")
            .size_full()
            .flex()
            .relative()
            .rounded(chrome_radius)
            .overflow_hidden()
            .bg(transparent_black())
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
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_toggle_snippet_palette))
            .on_action(cx.listener(Self::on_snippet_queue_paste))
            .on_action(cx.listener(Self::on_quit_requested))
            .child(window_surface)
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        if self.snippet_editor.is_some() {
            self.snippet_input_focus_handle.clone()
        } else {
            self.focus_handle.clone()
        }
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

fn env_gpu_acceleration_enabled() -> bool {
    matches!(
        std::env::var("ZWG_ENABLE_DX12_GPU").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("on") | Some("ON")
    )
}

fn terminal_settings_from_config(
    config: &AppConfig,
    input_suppressed: Arc<AtomicBool>,
) -> TerminalSettings {
    let theme = config.active_theme();
    TerminalSettings {
        cols: config.default_window_cols,
        rows: config.default_window_rows,
        scrollback_lines: config.scrollback_lines,
        font_family: config.font.family.trim().to_string(),
        font_size: config.font.size,
        cursor_blink: config.cursor_blink,
        copy_on_select: config.copy_on_select,
        gpu_acceleration: config.gpu_acceleration || env_gpu_acceleration_enabled(),
        fg_color: theme.fg,
        bg_color: theme.bg,
        background_image_path: config.background_image_path.clone(),
        background_image_opacity: config.background_image_opacity as f32 / 100.0,
        global_hotkeys: collect_global_hotkeys(config),
        input_suppressed,
    }
}

fn bind_global_key_bindings(cx: &mut App, config: &AppConfig, keyboard_capture_active: bool) {
    cx.clear_key_bindings();

    let mut bindings = Vec::new();
    if !keyboard_capture_active {
        push_optional_key_binding(&mut bindings, &config.keyboard.new_tab, NewTab);
        push_optional_key_binding(&mut bindings, &config.keyboard.close_tab, CloseTab);
        push_optional_key_binding(&mut bindings, &config.keyboard.split_right, SplitRight);
        push_optional_key_binding(&mut bindings, &config.keyboard.split_down, SplitDown);
        push_optional_key_binding(&mut bindings, &config.keyboard.close_pane, ClosePane);
        push_optional_key_binding(&mut bindings, &config.keyboard.focus_next_pane, FocusNext);
        push_optional_key_binding(&mut bindings, &config.keyboard.focus_prev_pane, FocusPrev);
        push_optional_key_binding(&mut bindings, &config.keyboard.open_settings, OpenSettings);
    }
    bindings.push(KeyBinding::new("ctrl-shift-v", ToggleSnippetPalette, None));
    bindings.push(KeyBinding::new("ctrl-shift-f", SnippetQueuePaste, None));
    bindings.push(KeyBinding::new("ctrl-shift-q", Quit, None));
    cx.bind_keys(bindings);
}

fn collect_global_hotkeys(config: &AppConfig) -> Vec<String> {
    let mut shortcuts = Vec::new();

    for shortcut in [
        config.keyboard.new_tab.as_str(),
        config.keyboard.close_tab.as_str(),
        config.keyboard.split_right.as_str(),
        config.keyboard.split_down.as_str(),
        config.keyboard.close_pane.as_str(),
        config.keyboard.focus_next_pane.as_str(),
        config.keyboard.focus_prev_pane.as_str(),
        config.keyboard.open_settings.as_str(),
        "Ctrl+Shift+V",
        "Ctrl+Shift+F",
        "Ctrl+Shift+Q",
    ] {
        let shortcut = shortcut.trim();
        if shortcut.is_empty() || shortcuts.iter().any(|existing| existing == shortcut) {
            continue;
        }
        shortcuts.push(shortcut.to_string());
    }

    shortcuts
}

fn configured_global_shortcut_action(
    event: &KeyDownEvent,
    config: &AppConfig,
) -> Option<GlobalShortcutAction> {
    for (shortcut, action) in [
        (
            config.keyboard.new_tab.as_str(),
            GlobalShortcutAction::NewTab,
        ),
        (
            config.keyboard.close_tab.as_str(),
            GlobalShortcutAction::CloseTab,
        ),
        (
            config.keyboard.split_right.as_str(),
            GlobalShortcutAction::SplitRight,
        ),
        (
            config.keyboard.split_down.as_str(),
            GlobalShortcutAction::SplitDown,
        ),
        (
            config.keyboard.close_pane.as_str(),
            GlobalShortcutAction::ClosePane,
        ),
        (
            config.keyboard.focus_next_pane.as_str(),
            GlobalShortcutAction::FocusNext,
        ),
        (
            config.keyboard.focus_prev_pane.as_str(),
            GlobalShortcutAction::FocusPrev,
        ),
        (
            config.keyboard.open_settings.as_str(),
            GlobalShortcutAction::OpenSettings,
        ),
        ("Ctrl+Shift+V", GlobalShortcutAction::ToggleSnippetPalette),
        ("Ctrl+Shift+F", GlobalShortcutAction::SnippetQueuePaste),
        ("Ctrl+Shift+Q", GlobalShortcutAction::Quit),
    ] {
        if hotkey_matches(event, shortcut) {
            return Some(action);
        }
    }

    None
}

fn push_optional_key_binding<A: Action>(bindings: &mut Vec<KeyBinding>, shortcut: &str, action: A) {
    if let Some(binding) = hotkey_binding_string(shortcut) {
        bindings.push(KeyBinding::new(binding.as_str(), action, None));
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

impl EntityInputHandler for RootView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let target = self.compute_snippet_ime_target()?;
        let text = self.current_snippet_ime_text(target)?;
        let range = utf16_range_to_byte_range(&text, &range_utf16);
        adjusted_range.replace(byte_range_to_utf16_range(&text, &range));
        Some(text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let target = self.compute_snippet_ime_target()?;
        let text = self.current_snippet_ime_text(target)?;
        let range = self.current_snippet_ime_selection_range(target)?;
        Some(UTF16Selection {
            range: byte_range_to_utf16_range(&text, &range),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let target = self.compute_snippet_ime_target()?;
        let text = self.current_snippet_ime_text(target)?;
        self.snippet_ime_marked_range
            .as_ref()
            .map(|range| byte_range_to_utf16_range(&text, range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.clear_snippet_ime_state();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.compute_snippet_ime_target() else {
            return;
        };
        let Some(current_text) = self.current_snippet_ime_text(target) else {
            return;
        };
        let range = range_utf16
            .as_ref()
            .map(|range| utf16_range_to_byte_range(&current_text, range))
            .or_else(|| self.snippet_ime_marked_range.clone())
            .or_else(|| self.current_snippet_ime_selection_range(target))
            .unwrap_or_else(|| current_text.len()..current_text.len());
        SNIPPET_IME_VK_PROCESSKEY.store(false, Ordering::Release);
        if let Some(inserted) = self.replace_text_in_active_snippet_ime_target(target, range, text)
        {
            self.snippet_ime_marked_range = None;
            self.snippet_ime_selected_range = Some(inserted.end..inserted.end);
            if self.compute_snippet_ime_target() == Some(SnippetImeTarget::PaletteQuery) {
                self.request_ai_suggestions(cx);
            }
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.compute_snippet_ime_target() else {
            return;
        };
        let Some(current_text) = self.current_snippet_ime_text(target) else {
            return;
        };
        let range = range_utf16
            .as_ref()
            .map(|value| utf16_range_to_byte_range(&current_text, value))
            .or_else(|| self.snippet_ime_marked_range.clone())
            .or_else(|| self.current_snippet_ime_selection_range(target))
            .unwrap_or_else(|| current_text.len()..current_text.len());
        SNIPPET_IME_VK_PROCESSKEY.store(false, Ordering::Release);
        if let Some(inserted) =
            self.replace_text_in_active_snippet_ime_target(target, range.clone(), new_text)
        {
            self.snippet_ime_marked_range = if new_text.is_empty() {
                None
            } else {
                Some(inserted.clone())
            };
            self.snippet_ime_selected_range = new_selected_range_utf16
                .as_ref()
                .map(|value| utf16_range_to_byte_range(new_text, value))
                .map(|value| inserted.start + value.start..inserted.start + value.end)
                .or_else(|| Some(inserted.end..inserted.end));
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(
            point(
                element_bounds.left() + px(12.0),
                element_bounds.top() + px(8.0),
            ),
            size(px(2.0), px(20.0)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

fn snippet_ime_input_overlay(entity: Entity<RootView>, focus_handle: FocusHandle) -> AnyElement {
    canvas(
        |_bounds, _window, _cx| {},
        move |bounds, _, window, cx| {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, entity.clone()),
                cx,
            );
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

fn chrome_button(id: &'static str, icon_path: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(24.0))
        .min_w(px(24.0))
        .h(px(24.0))
        .flex_shrink_0()
        .rounded(px(6.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .font_family(UI_FONT)
        .text_size(px(12.0))
        .hover(|style| style.bg(rgba(0xffffff16)))
        .child(
            svg()
                .path(icon_path)
                .size(px(14.0))
                .text_color(rgb(TEXT_SOFT)),
        )
}

fn titlebar_actions_width(show_snippets_button: bool) -> f32 {
    let action_slots: usize = if show_snippets_button { 4 } else { 3 };
    let button_width = 24.0;
    let button_gap = 4.0;

    (action_slots as f32 * button_width) + ((action_slots.saturating_sub(1) as f32) * button_gap)
}

fn traffic_lights_width() -> f32 {
    let light_count = 3.0;
    let light_width = 12.0;
    let light_gap = 8.0;

    (light_count * light_width) + ((light_count - 1.0) * light_gap)
}

fn titlebar_side_cluster_width(show_snippets_button: bool) -> f32 {
    titlebar_actions_width(show_snippets_button).max(traffic_lights_width())
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

fn render_snippet_notice(notice: Option<&SnippetNotice>, position: &str) -> Option<AnyElement> {
    let notice = notice?;
    let (top, left, right, bottom) = match position {
        "topLeft" => (16.0, Some(16.0), None, None),
        "bottomLeft" => (0.0, Some(16.0), None, Some(16.0)),
        "bottomRight" => (0.0, None, Some(16.0), Some(16.0)),
        _ => (16.0, None, Some(16.0), None),
    };
    let mut card = div()
        .id("snippet-notice")
        .absolute()
        .w(px(280.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(rgba(0xBFDBFEFF))
        .bg(rgba(0xEFF6FFFF))
        .shadow_lg()
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(4.0));

    if let Some(right) = right {
        card = card.right(px(right));
    }
    if let Some(left) = left {
        card = card.left(px(left));
    }
    if let Some(bottom) = bottom {
        card = card.bottom(px(bottom));
    } else {
        card = card.top(px(top));
    }

    Some(
        card.child(
            div()
                .font_family(UI_FONT)
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgba(0x1D4ED8FF))
                .child(notice.title.clone()),
        )
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(11.0))
                .text_color(rgba(0x334155FF))
                .child(notice.detail.clone()),
        )
        .into_any_element(),
    )
}

fn render_app_notice(notice: Option<&AppNotice>) -> Option<AnyElement> {
    let notice = notice?;

    Some(
        div()
            .id("app-notice")
            .absolute()
            .top(px(16.0))
            .right(px(16.0))
            .w(px(300.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(rgba(0xBFDBFEFF))
            .bg(rgba(0xEFF6FFFF))
            .shadow_lg()
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgba(0x1D4ED8FF))
                    .child(notice.title.clone()),
            )
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(11.0))
                    .text_color(rgba(0x334155FF))
                    .child(notice.detail.clone()),
            )
            .into_any_element(),
    )
}

#[cfg(target_os = "windows")]
fn play_notification_bell() {
    print!("\x07");
    let _ = std::io::stdout().flush();
}

#[cfg(not(target_os = "windows"))]
fn play_notification_bell() {
    print!("\x07");
    let _ = std::io::stdout().flush();
}

fn process_completion_notice_detail(tab_title: &str, exit_code: i32) -> String {
    if exit_code == 0 {
        format!("{tab_title} のプロセスが正常終了しました。")
    } else {
        format!("{tab_title} のプロセスが終了コード {exit_code} で終了しました。")
    }
}

fn snippet_modal_origin(
    position: &str,
    viewport_w: f32,
    viewport_h: f32,
    panel_w: f32,
    panel_h: f32,
) -> (f32, f32) {
    match position {
        "topLeft" => (24.0, 24.0),
        "topRight" => (24.0, (viewport_w - panel_w - 24.0).max(24.0)),
        "bottomLeft" => ((viewport_h - panel_h - 24.0).max(24.0), 24.0),
        "bottomRight" => (
            (viewport_h - panel_h - 24.0).max(24.0),
            (viewport_w - panel_w - 24.0).max(24.0),
        ),
        _ => (
            ((viewport_h - panel_h) * 0.5).max(24.0),
            ((viewport_w - panel_w) * 0.5).max(24.0),
        ),
    }
}

fn light_settings_tab(
    label: &'static str,
    active: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w_full()
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .font_family(UI_FONT)
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active {
            rgba(0xFFFFFFFF)
        } else {
            rgba(0x374151FF)
        })
        .bg(if active {
            rgba(0x3B82F6FF)
        } else {
            rgba(0x00000000)
        })
        .hover(|style| style.bg(rgba(0xE5E7EBFF)))
        .on_mouse_down(MouseButton::Left, listener)
        .child(label)
}

fn light_section_title(label: &'static str) -> Div {
    div()
        .font_family(UI_FONT)
        .text_size(px(18.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgba(0x1F2937FF))
        .child(label)
}

fn light_section_subtitle(label: &'static str) -> Div {
    div()
        .font_family(UI_FONT)
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgba(0x374151FF))
        .child(label)
}

fn light_toggle_row(
    title: &'static str,
    description: impl Into<String>,
    checked: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    light_form_row(title, description, light_toggle_control(checked, listener))
}

fn light_cycle_row(
    title: &'static str,
    description: impl Into<String>,
    value: impl Into<String>,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    light_form_row(title, description, light_cycle_box(value.into(), listener))
}

fn light_slider_row(
    title: &'static str,
    description: impl Into<String>,
    value: impl Into<String>,
    ratio: f32,
    minus_listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    plus_listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    light_form_row(
        title,
        description,
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(light_square_button("-", minus_listener))
            .child(light_slider_bar(ratio, value.into()))
            .child(light_square_button("+", plus_listener)),
    )
}

fn light_editable_row(
    title: &'static str,
    description: impl Into<String>,
    value: impl Into<String>,
    active: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    light_form_row(
        title,
        description,
        light_text_input_box(value.into(), "", active, listener),
    )
}

fn light_editable_multiline_row(
    title: &'static str,
    description: impl Into<String>,
    value: impl Into<String>,
    active: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    light_form_row(
        title,
        description,
        light_text_area_box(value.into(), active, listener),
    )
}

fn light_form_row(
    title: &'static str,
    description: impl Into<String>,
    control: impl IntoElement,
) -> Div {
    div()
        .w_full()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgba(0xE5E7EBFF))
        .bg(rgb(0xFFFFFF))
        .p(px(14.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
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
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgba(0x1F2937FF))
                        .child(title),
                )
                .child(
                    div()
                        .font_family(UI_FONT)
                        .text_size(px(11.0))
                        .text_color(rgba(0x6B7280FF))
                        .child(description.into()),
                ),
        )
        .child(control)
}

fn light_toggle_control(
    checked: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(38.0))
        .h(px(22.0))
        .rounded_full()
        .cursor_pointer()
        .bg(if checked {
            rgba(0x111827FF)
        } else {
            rgba(0xD1D5DBFF)
        })
        .p(px(2.0))
        .flex()
        .justify_start()
        .when(checked, |style| style.justify_end())
        .on_mouse_down(MouseButton::Left, listener)
        .child(
            div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(rgb(0xFFFFFF)),
        )
}

fn light_cycle_box(
    value: String,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .min_w(px(140.0))
        .h(px(34.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(rgba(0xD1D5DBFF))
        .bg(rgb(0xF9FAFB))
        .px(px(12.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_between()
        .hover(|style| style.bg(rgba(0xF3F4F6FF)))
        .on_mouse_down(MouseButton::Left, listener)
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(12.0))
                .text_color(rgba(0x374151FF))
                .child(value),
        )
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(10.0))
                .text_color(rgba(0x9CA3AFFF))
                .child("v"),
        )
}

fn light_text_input_box(
    value: String,
    placeholder: &'static str,
    active: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .min_w(px(180.0))
        .h(px(34.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(if active {
            rgba(0x60A5FAFF)
        } else {
            rgba(0xD1D5DBFF)
        })
        .bg(rgb(0xFFFFFF))
        .px(px(12.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .on_mouse_down(MouseButton::Left, listener)
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(12.0))
                .text_color(if value.is_empty() {
                    rgba(0x9CA3AFFF)
                } else {
                    rgba(0x111827FF)
                })
                .child(if value.is_empty() {
                    placeholder.to_string()
                } else {
                    value
                }),
        )
}

fn light_text_area_box(
    value: String,
    active: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(260.0))
        .min_h(px(72.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(if active {
            rgba(0x60A5FAFF)
        } else {
            rgba(0xD1D5DBFF)
        })
        .bg(rgb(0xFFFFFF))
        .p(px(10.0))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, listener)
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(12.0))
                .text_color(if value.is_empty() {
                    rgba(0x9CA3AFFF)
                } else {
                    rgba(0x111827FF)
                })
                .child(if value.is_empty() {
                    "例: 1Password, KeePassXC".to_string()
                } else {
                    value
                }),
        )
}

fn light_square_button(
    label: &'static str,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(28.0))
        .h(px(28.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(rgba(0xD1D5DBFF))
        .bg(rgb(0xFFFFFF))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .hover(|style| style.bg(rgba(0xF3F4F6FF)))
        .on_mouse_down(MouseButton::Left, listener)
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgba(0x374151FF))
                .child(label),
        )
}

fn light_slider_bar(ratio: f32, value: String) -> Div {
    let fill = ratio.clamp(0.0, 1.0) * 120.0;
    div()
        .w(px(120.0))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(11.0))
                .text_color(rgba(0x6B7280FF))
                .child(value),
        )
        .child(
            div()
                .w(px(120.0))
                .h(px(8.0))
                .rounded_full()
                .bg(rgba(0xE5E7EBFF))
                .child(
                    div()
                        .w(px(fill))
                        .h(px(8.0))
                        .rounded_full()
                        .bg(rgba(0x111827FF)),
                ),
        )
}

fn light_primary_button(
    label: &'static str,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .px(px(14.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(rgba(0x111827FF))
        .cursor_pointer()
        .font_family(UI_FONT)
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgba(0xFFFFFFFF))
        .hover(|style| style.bg(rgba(0x1F2937FF)))
        .on_mouse_down(MouseButton::Left, listener)
        .child(label)
}

fn light_secondary_button(
    label: &'static str,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .px(px(14.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(rgba(0xD1D5DBFF))
        .bg(rgb(0xFFFFFF))
        .cursor_pointer()
        .font_family(UI_FONT)
        .text_size(px(12.0))
        .text_color(rgba(0x374151FF))
        .hover(|style| style.bg(rgba(0xF9FAFBFF)))
        .on_mouse_down(MouseButton::Left, listener)
        .child(label)
}

fn light_outline_button(
    label: &'static str,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    light_secondary_button(label, listener)
}

fn light_info_box(message: &'static str) -> Div {
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgba(0xBFDBFEFF))
        .bg(rgba(0xEFF6FFFF))
        .p(px(12.0))
        .font_family(UI_FONT)
        .text_size(px(11.0))
        .text_color(rgba(0x1D4ED8FF))
        .child(message)
}

fn light_warning_box(message: &'static str) -> Div {
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgba(0xFDE68AFF))
        .bg(rgba(0xFEFCE8FF))
        .p(px(12.0))
        .font_family(UI_FONT)
        .text_size(px(11.0))
        .text_color(rgba(0x92400EFF))
        .child(message)
}

fn light_pattern_chip(
    pattern: String,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w_full()
        .rounded(px(8.0))
        .border_1()
        .border_color(rgba(0xE5E7EBFF))
        .bg(rgb(0xF9FAFB))
        .px(px(10.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .font_family(MONO_FONT)
                .text_size(px(11.0))
                .text_color(rgba(0x374151FF))
                .child(pattern),
        )
        .child(light_square_button("x", listener))
}

fn light_template_card(
    title: String,
    group: String,
    preview: String,
    favorite: bool,
    edit_listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    delete_listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgba(0xE5E7EBFF))
        .bg(rgb(0xFFFFFF))
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
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
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgba(0x111827FF))
                                .child(if favorite {
                                    format!("{}  *", title)
                                } else {
                                    title
                                }),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(10.0))
                                .text_color(rgba(0x6B7280FF))
                                .child(group),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(light_secondary_button("編集", edit_listener))
                        .child(
                            div()
                                .px(px(12.0))
                                .py(px(8.0))
                                .rounded(px(8.0))
                                .bg(rgba(0xDC2626FF))
                                .cursor_pointer()
                                .font_family(UI_FONT)
                                .text_size(px(12.0))
                                .text_color(rgba(0xFFFFFFFF))
                                .hover(|style| style.bg(rgba(0xB91C1CFF)))
                                .on_mouse_down(MouseButton::Left, delete_listener)
                                .child("削除"),
                        ),
                ),
        )
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(11.0))
                .text_color(rgba(0x4B5563FF))
                .child(preview),
        )
}

fn light_preview_box(title: &'static str, body: &'static str, sub: &'static str) -> Div {
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgba(0xE5E7EBFF))
        .bg(rgb(0xF9FAFB))
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(light_section_subtitle(title))
        .child(
            div()
                .rounded(px(10.0))
                .border_1()
                .border_color(rgba(0xBFDBFEFF))
                .bg(rgb(0xFFFFFF))
                .p(px(12.0))
                .child(
                    div()
                        .font_family(UI_FONT)
                        .text_size(px(12.0))
                        .text_color(rgba(0x111827FF))
                        .child(body),
                )
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(px(11.0))
                        .text_color(rgba(0x6B7280FF))
                        .child(sub),
                ),
        )
}

fn light_action_card(
    title: &'static str,
    description: &'static str,
    button_label: &'static str,
    destructive: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(if destructive {
            rgba(0xFECACAFF)
        } else {
            rgba(0xE5E7EBFF)
        })
        .bg(rgb(0xFFFFFF))
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(light_section_subtitle(title))
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(11.0))
                .text_color(if destructive {
                    rgba(0xB91C1CFF)
                } else {
                    rgba(0x6B7280FF)
                })
                .child(description),
        )
        .child(if destructive {
            div()
                .px(px(14.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(rgba(0xDC2626FF))
                .cursor_pointer()
                .font_family(UI_FONT)
                .text_size(px(12.0))
                .text_color(rgba(0xFFFFFFFF))
                .hover(|style| style.bg(rgba(0xB91C1CFF)))
                .on_mouse_down(MouseButton::Left, listener)
                .child(button_label)
        } else {
            light_secondary_button(button_label, listener)
        })
}

fn light_storage_card(items: usize, favorites: usize, queued: usize, bytes: u64) -> Div {
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgba(0xE5E7EBFF))
        .bg(rgb(0xF9FAFB))
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(light_section_subtitle("ストレージ情報"))
        .child(light_storage_row("定型文数", items.to_string()))
        .child(light_storage_row("お気に入り数", favorites.to_string()))
        .child(light_storage_row("キュー件数", queued.to_string()))
        .child(light_storage_row("使用容量", format_bytes(bytes)))
}

fn light_storage_row(label: &'static str, value: String) -> Div {
    div()
        .w_full()
        .flex()
        .justify_between()
        .font_family(UI_FONT)
        .text_size(px(11.0))
        .text_color(rgba(0x4B5563FF))
        .child(label)
        .child(value)
}

fn light_empty_state(message: &'static str) -> Div {
    div()
        .w_full()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgba(0xE5E7EBFF))
        .bg(rgb(0xFFFFFF))
        .p(px(24.0))
        .font_family(UI_FONT)
        .text_size(px(12.0))
        .text_color(rgba(0x9CA3AFFF))
        .child(message)
}

fn theme_label(theme: &str) -> String {
    match theme {
        "dark" => "ダーク".to_string(),
        "auto" => "自動".to_string(),
        _ => "ライト".to_string(),
    }
}

fn window_position_label(position: &str) -> String {
    match position {
        "cursor" => "カーソル位置".to_string(),
        "topLeft" => "左上".to_string(),
        "topRight" => "右上".to_string(),
        "bottomLeft" => "左下".to_string(),
        "bottomRight" => "右下".to_string(),
        _ => "中央".to_string(),
    }
}

fn notification_position_label(position: &str) -> String {
    match position {
        "topLeft" => "左上".to_string(),
        "bottomLeft" => "左下".to_string(),
        "bottomRight" => "右下".to_string(),
        _ => "右上".to_string(),
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn rgba_with_alpha(rgb: u32, alpha_percent: u8) -> u32 {
    let alpha = ((alpha_percent as u32) * 255 / 100) & 0xFF;
    (rgb << 8) | alpha
}

pub(crate) fn hotkey_matches(event: &KeyDownEvent, hotkey: &str) -> bool {
    if hotkey.trim().is_empty() {
        return false;
    }

    let mut require_ctrl = false;
    let mut require_shift = false;
    let mut require_alt = false;
    let mut key = String::new();

    for part in hotkey.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" => require_ctrl = true,
            "shift" => require_shift = true,
            "alt" | "option" => require_alt = true,
            other => key = canonical_hotkey_key(other),
        }
    }

    if key.is_empty() {
        return false;
    }

    let modifiers = &event.keystroke.modifiers;
    let alt_pressed = modifiers.alt;
    require_ctrl == modifiers.control
        && require_shift == modifiers.shift
        && require_alt == alt_pressed
        && canonical_hotkey_key(&event.keystroke.key) == key
}

fn hotkey_binding_string(hotkey: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut key = None;

    for part in hotkey.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" => parts.push("ctrl".to_string()),
            "shift" => parts.push("shift".to_string()),
            "alt" | "option" => parts.push("alt".to_string()),
            other => key = Some(canonical_hotkey_key(other)),
        }
    }

    let key = key?;
    if parts.is_empty() {
        return None;
    }
    parts.push(key);
    Some(parts.join("-"))
}

fn hotkey_string_for_keystroke(keystroke: &Keystroke) -> Option<String> {
    if !keystroke.modifiers.control && !keystroke.modifiers.alt && !keystroke.modifiers.shift {
        return None;
    }

    let key = canonical_hotkey_key(&keystroke.key);
    if key.is_empty() || is_modifier_only_key(&key) {
        return None;
    }

    let mut parts = Vec::new();
    if keystroke.modifiers.control {
        parts.push("Ctrl".to_string());
    }
    if keystroke.modifiers.alt {
        parts.push("Alt".to_string());
    }
    if keystroke.modifiers.shift {
        parts.push("Shift".to_string());
    }
    parts.push(display_hotkey_key(&key));
    Some(parts.join("+"))
}

fn canonical_hotkey_key(key: &str) -> String {
    match key.trim().to_ascii_lowercase().as_str() {
        "," | "comma" => "comma".to_string(),
        "." | "period" => "period".to_string(),
        " " | "space" => "space".to_string(),
        "esc" => "escape".to_string(),
        "return" => "enter".to_string(),
        other => other.to_string(),
    }
}

fn is_modifier_only_key(key: &str) -> bool {
    matches!(
        key,
        "ctrl" | "control" | "shift" | "alt" | "option" | "cmd" | "command" | "super" | "win"
    )
}

fn display_hotkey_key(key: &str) -> String {
    match key {
        "comma" => "Comma".to_string(),
        "period" => "Period".to_string(),
        "space" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "enter" => "Enter".to_string(),
        "escape" => "Escape".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        _ if key.len() == 1 => key.to_ascii_uppercase(),
        _ => {
            let mut chars = key.chars();
            match chars.next() {
                Some(first) => {
                    let mut display = String::new();
                    display.push(first.to_ascii_uppercase());
                    display.extend(chars);
                    display
                }
                None => String::new(),
            }
        }
    }
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
        input = input.font_family(MONO_FONT);
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
        input = input.font_family(MONO_FONT);
    } else {
        input = input.font_family(UI_FONT);
    }

    input
}

fn interactive_input_box_dynamic(
    value: String,
    placeholder: &'static str,
    width: f32,
    mono: bool,
    active: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let display_value = if value.is_empty() {
        placeholder.to_string()
    } else {
        value
    };
    let mut input = div()
        .w(px(width))
        .h(px(32.0))
        .rounded(px(8.0))
        .bg(rgba(0xffffff10))
        .border_1()
        .border_color(if active {
            rgba(0x60A5FAFF)
        } else {
            rgba(0xffffff10)
        })
        .px(px(12.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_start()
        .hover(|style| style.bg(rgba(0xffffff14)))
        .on_mouse_down(MouseButton::Left, listener)
        .text_size(px(13.0))
        .text_color(if display_value == placeholder {
            rgb(SUBTEXT1)
        } else {
            rgb(TEXT)
        })
        .child(display_value);

    if mono {
        input = input.font_family(MONO_FONT);
    } else {
        input = input.font_family(UI_FONT);
    }

    input
}

fn interactive_cycle_box_dynamic(
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
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_between()
        .hover(|style| style.bg(rgba(0xffffff14)))
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

fn slider_with_dynamic_value(fill_ratio: f32, value: String) -> Div {
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
                .w(px(48.0))
                .font_family(UI_FONT)
                .text_size(px(13.0))
                .text_color(rgb(SUBTEXT1))
                .text_right()
                .child(value),
        )
}

fn interactive_slider_with_value(
    fill_ratio: f32,
    value: String,
    on_decrement: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_increment: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(stepper_button("-", on_decrement))
        .child(slider_with_dynamic_value(fill_ratio, value))
        .child(stepper_button("+", on_increment))
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

#[cfg(test)]
mod tests {
    use super::{
        GlobalShortcutAction, SnippetEditorField, SnippetEditorMode, SnippetEditorState,
        SnippetGroupEditMode, SnippetGroupEditorState, SnippetImeTarget, TextCursorMove,
        ZoomAction, adjust_font_size_value, append_text_to_snippet_editor_field,
        append_text_to_snippet_ime_target, apply_snippet_group_edit, begin_new_snippet_group_edit,
        byte_index_to_utf16_offset, byte_range_to_utf16_range, collect_global_hotkeys,
        configured_global_shortcut_action, cycle_string_option, should_defer_snippet_keystroke_to_ime,
        delete_text_before_snippet_editor_cursor, direct_text_from_snippet_keystroke,
        hotkey_binding_string, hotkey_matches, hotkey_string_for_keystroke,
        move_snippet_editor_cursor, paste_text_to_snippet_editor_field, SNIPPET_IME_VK_PROCESSKEY,
        process_completion_notice_detail, terminal_settings_from_config, titlebar_actions_width,
        titlebar_side_cluster_width, traffic_lights_width, utf16_offset_to_byte_index,
        utf16_range_to_byte_range, zoom_action_for_window,
    };
    use crate::config::AppConfig;
    use crate::snippets::{SnippetPalette, SnippetStore};
    use gpui::{KeyDownEvent, Keystroke, Modifiers};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn zoom_action_maximizes_when_windowed() {
        assert_eq!(zoom_action_for_window(false), ZoomAction::Maximize);
    }

    #[test]
    fn zoom_action_restores_when_already_maximized() {
        assert_eq!(zoom_action_for_window(true), ZoomAction::Restore);
    }

    #[test]
    fn titlebar_drag_strategy_matches_platform_support() {
        #[cfg(target_os = "windows")]
        assert!(
            cfg!(target_os = "windows"),
            "Windows should post a system move message when client drag fallback runs"
        );

        #[cfg(not(target_os = "windows"))]
        assert!(
            !cfg!(target_os = "windows"),
            "Non-Windows platforms use start_window_move directly"
        );
    }

    #[test]
    fn titlebar_actions_width_reserves_space_for_optional_snippets_button() {
        assert_eq!(titlebar_actions_width(false), 80.0);
        assert_eq!(titlebar_actions_width(true), 108.0);
    }

    #[test]
    fn titlebar_side_cluster_width_tracks_wider_control_group() {
        assert_eq!(traffic_lights_width(), 52.0);
        assert_eq!(titlebar_side_cluster_width(false), 80.0);
        assert_eq!(titlebar_side_cluster_width(true), 108.0);
    }

    #[test]
    fn begin_new_group_edit_appends_placeholder_without_overwriting_existing_groups() {
        let path = temp_snippet_store_path();
        let store = SnippetStore::load_from_path(&path);
        let mut palette = SnippetPalette::new(store);
        let original_groups = palette.groups().to_vec();
        let mut editor = SnippetGroupEditorState::default();

        assert!(begin_new_snippet_group_edit(&mut palette, &mut editor));

        assert_eq!(palette.groups().len(), original_groups.len() + 1);
        assert_eq!(
            &palette.groups()[..original_groups.len()],
            original_groups.as_slice()
        );
        assert_eq!(
            palette.group_name_at(original_groups.len()),
            Some("New Group")
        );
        assert_eq!(palette.active_group(), Some("New Group"));
        assert_eq!(editor.selected_index, original_groups.len());
        assert_eq!(
            editor.edit_mode,
            SnippetGroupEditMode::New(original_groups.len())
        );
        assert_eq!(editor.draft_name, "New Group");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn applying_new_group_edit_preserves_existing_groups_and_allows_multiple_additions() {
        let path = temp_snippet_store_path();
        let store = SnippetStore::load_from_path(&path);
        let mut palette = SnippetPalette::new(store);
        let original_groups = palette.groups().to_vec();
        let mut editor = SnippetGroupEditorState::default();

        assert!(begin_new_snippet_group_edit(&mut palette, &mut editor));
        editor.draft_name = "Ops".to_string();
        assert!(apply_snippet_group_edit(&mut palette, &mut editor));

        assert!(begin_new_snippet_group_edit(&mut palette, &mut editor));
        editor.draft_name = "Docs".to_string();
        assert!(apply_snippet_group_edit(&mut palette, &mut editor));

        assert_eq!(palette.groups().len(), original_groups.len() + 2);
        assert_eq!(
            &palette.groups()[..original_groups.len()],
            original_groups.as_slice()
        );
        assert!(palette.group_index_by_name("Ops").is_some());
        assert!(palette.group_index_by_name("Docs").is_some());
        assert_eq!(editor.edit_mode, SnippetGroupEditMode::None);
        assert!(editor.draft_name.is_empty());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn snippet_editor_text_input_updates_title_field() {
        let mut editor = SnippetEditorState {
            mode: SnippetEditorMode::Add,
            title: String::new(),
            title_cursor: 0,
            content: String::new(),
            content_cursor: 0,
            group_index: 0,
            active_field: SnippetEditorField::Title,
        };

        assert!(append_text_to_snippet_editor_field(&mut editor, "abc"));
        assert_eq!(editor.title, "abc");
        assert!(editor.content.is_empty());
    }

    #[test]
    fn snippet_editor_text_input_updates_content_field() {
        let mut editor = SnippetEditorState {
            mode: SnippetEditorMode::Add,
            title: String::from("title"),
            title_cursor: "title".len(),
            content: String::new(),
            content_cursor: 0,
            group_index: 0,
            active_field: SnippetEditorField::Content,
        };

        assert!(append_text_to_snippet_editor_field(&mut editor, "body"));
        assert_eq!(editor.title, "title");
        assert_eq!(editor.content, "body");
    }

    #[test]
    fn snippet_ime_commit_appends_text_to_palette_query() {
        let path = temp_snippet_store_path();
        let store = SnippetStore::load_from_path(&path);
        let mut palette = SnippetPalette::new(store);

        assert!(append_text_to_snippet_ime_target(
            SnippetImeTarget::PaletteQuery,
            &mut palette,
            None,
            None,
            "かな",
        ));
        assert_eq!(palette.query(), "かな");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn snippet_ime_commit_appends_text_to_active_editor_field() {
        let path = temp_snippet_store_path();
        let store = SnippetStore::load_from_path(&path);
        let mut palette = SnippetPalette::new(store);
        let mut editor = SnippetEditorState {
            mode: SnippetEditorMode::Add,
            title: String::from("title"),
            title_cursor: "title".len(),
            content: String::new(),
            content_cursor: 0,
            group_index: 0,
            active_field: SnippetEditorField::Content,
        };

        assert!(append_text_to_snippet_ime_target(
            SnippetImeTarget::EditorContent,
            &mut palette,
            None,
            Some(&mut editor),
            "日本語",
        ));
        assert_eq!(editor.title, "title");
        assert_eq!(editor.content, "日本語");
        assert_eq!(editor.content_cursor, "日本語".len());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn snippet_editor_text_input_inserts_at_cursor_position() {
        let mut editor = SnippetEditorState {
            mode: SnippetEditorMode::Edit(0),
            title: String::from("ac"),
            title_cursor: 1,
            content: String::new(),
            content_cursor: 0,
            group_index: 0,
            active_field: SnippetEditorField::Title,
        };

        assert!(append_text_to_snippet_editor_field(&mut editor, "b"));
        assert_eq!(editor.title, "abc");
        assert_eq!(editor.title_cursor, 2);
    }

    #[test]
    fn direct_snippet_text_from_keystroke_prefers_key_char() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "process".into(),
            key_char: Some("日本".into()),
        };

        assert_eq!(
            direct_text_from_snippet_keystroke(&keystroke).as_deref(),
            Some("日本")
        );
    }

    #[test]
    fn direct_snippet_text_from_keystroke_falls_back_to_single_key() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "a".into(),
            key_char: None,
        };

        assert_eq!(
            direct_text_from_snippet_keystroke(&keystroke).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn direct_snippet_text_from_keystroke_falls_back_to_single_unicode_key() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "あ".into(),
            key_char: None,
        };

        assert_eq!(
            direct_text_from_snippet_keystroke(&keystroke).as_deref(),
            Some("あ")
        );
    }

    #[test]
    fn should_defer_snippet_keystroke_to_ime_skips_when_not_text() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "process".into(),
            key_char: None,
        };

        SNIPPET_IME_VK_PROCESSKEY.store(true, Ordering::Release);
        assert!(should_defer_snippet_keystroke_to_ime(&keystroke));
    }

    #[test]
    fn should_defer_snippet_keystroke_to_ime_allows_single_text_key() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "あ".into(),
            key_char: None,
        };

        SNIPPET_IME_VK_PROCESSKEY.store(true, Ordering::Release);
        assert!(!should_defer_snippet_keystroke_to_ime(&keystroke));
    }

    #[test]
    fn should_defer_snippet_keystroke_to_ime_defers_ascii_key_input() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "a".into(),
            key_char: Some("a".into()),
        };

        SNIPPET_IME_VK_PROCESSKEY.store(true, Ordering::Release);
        assert!(should_defer_snippet_keystroke_to_ime(&keystroke));
    }

    #[test]
    fn utf16_range_helpers_round_trip_multibyte_text() {
        let text = "Aあ😀B";
        let byte_range = "Aあ".len().."Aあ😀".len();
        let utf16_range = byte_range_to_utf16_range(text, &byte_range);

        assert_eq!(utf16_range, 2..4);
        assert_eq!(utf16_range_to_byte_range(text, &utf16_range), byte_range);
        assert_eq!(byte_index_to_utf16_offset(text, "Aあ😀".len()), 4);
        assert_eq!(utf16_offset_to_byte_index(text, 4), "Aあ😀".len());
    }

    #[test]
    fn snippet_editor_title_paste_accepts_japanese_text() {
        let mut editor = SnippetEditorState {
            mode: SnippetEditorMode::Edit(0),
            title: String::from("定文"),
            title_cursor: "定".len(),
            content: String::new(),
            content_cursor: 0,
            group_index: 0,
            active_field: SnippetEditorField::Title,
        };

        assert!(paste_text_to_snippet_editor_field(&mut editor, "型"));
        assert_eq!(editor.title, "定型文");
        assert_eq!(editor.title_cursor, "定型".len());
    }

    #[test]
    fn snippet_editor_content_paste_preserves_newlines() {
        let mut editor = SnippetEditorState {
            mode: SnippetEditorMode::Edit(0),
            title: String::from("title"),
            title_cursor: "title".len(),
            content: String::from("ab"),
            content_cursor: 1,
            group_index: 0,
            active_field: SnippetEditorField::Content,
        };

        assert!(paste_text_to_snippet_editor_field(
            &mut editor,
            "日本\r\n語"
        ));
        assert_eq!(editor.content, "a日本\n語b");
        assert_eq!(editor.content_cursor, "a日本\n語".len());
    }

    #[test]
    fn snippet_editor_title_paste_flattens_newlines() {
        let mut editor = SnippetEditorState {
            mode: SnippetEditorMode::Edit(0),
            title: String::new(),
            title_cursor: 0,
            content: String::new(),
            content_cursor: 0,
            group_index: 0,
            active_field: SnippetEditorField::Title,
        };

        assert!(paste_text_to_snippet_editor_field(
            &mut editor,
            "日本\r\n語"
        ));
        assert_eq!(editor.title, "日本 語");
        assert_eq!(editor.title_cursor, "日本 語".len());
    }

    #[test]
    fn snippet_editor_cursor_moves_and_backspace_follow_cursor_position() {
        let mut editor = SnippetEditorState {
            mode: SnippetEditorMode::Edit(0),
            title: String::from("abc"),
            title_cursor: 3,
            content: String::new(),
            content_cursor: 0,
            group_index: 0,
            active_field: SnippetEditorField::Title,
        };

        assert!(move_snippet_editor_cursor(
            &mut editor,
            TextCursorMove::Left
        ));
        assert_eq!(editor.title_cursor, 2);
        assert!(delete_text_before_snippet_editor_cursor(&mut editor));
        assert_eq!(editor.title, "ac");
        assert_eq!(editor.title_cursor, 1);
        assert!(move_snippet_editor_cursor(
            &mut editor,
            TextCursorMove::Home
        ));
        assert_eq!(editor.title_cursor, 0);
        assert!(move_snippet_editor_cursor(&mut editor, TextCursorMove::End));
        assert_eq!(editor.title_cursor, editor.title.len());
    }

    #[test]
    fn snippet_ime_commit_inserts_text_at_editor_cursor() {
        let path = temp_snippet_store_path();
        let store = SnippetStore::load_from_path(&path);
        let mut palette = SnippetPalette::new(store);
        let mut editor = SnippetEditorState {
            mode: SnippetEditorMode::Edit(0),
            title: String::from("ab"),
            title_cursor: 1,
            content: String::new(),
            content_cursor: 0,
            group_index: 0,
            active_field: SnippetEditorField::Title,
        };

        assert!(append_text_to_snippet_ime_target(
            SnippetImeTarget::EditorTitle,
            &mut palette,
            None,
            Some(&mut editor),
            "X",
        ));
        assert_eq!(editor.title, "aXb");
        assert_eq!(editor.title_cursor, 2);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn cycle_string_option_wraps_around() {
        let options = ["A", "B", "C"];
        assert_eq!(cycle_string_option("A", &options, -1), "C");
        assert_eq!(cycle_string_option("C", &options, 1), "A");
        assert_eq!(cycle_string_option("B", &options, 1), "C");
    }

    #[test]
    fn adjust_font_size_value_clamps_to_supported_range() {
        assert_eq!(adjust_font_size_value(6.0, -5), 6.0);
        assert_eq!(adjust_font_size_value(72.0, 5), 72.0);
        assert_eq!(adjust_font_size_value(14.0, 2), 16.0);
    }

    #[test]
    fn process_completion_notice_detail_mentions_exit_code_when_non_zero() {
        assert_eq!(
            process_completion_notice_detail("PowerShell", 0),
            "PowerShell のプロセスが正常終了しました。"
        );
        assert_eq!(
            process_completion_notice_detail("PowerShell", 12),
            "PowerShell のプロセスが終了コード 12 で終了しました。"
        );
    }

    #[test]
    fn terminal_settings_from_config_uses_active_theme_and_font_values() {
        let mut config = AppConfig::default();
        config.theme = "Tokyo Night".into();
        config.font.family = "Cascadia Mono".into();
        config.font.size = 16.0;
        config.background_image_path = Some("C:/images/tokyo.png".into());
        config.background_image_opacity = 42;
        config.cursor_blink = false;
        config.copy_on_select = true;
        config.gpu_acceleration = true;
        config.scrollback_lines = 2048;
        config.default_window_cols = 132;
        config.default_window_rows = 42;
        let input_suppressed = Arc::new(AtomicBool::new(true));

        let settings = terminal_settings_from_config(&config, input_suppressed.clone());

        assert_eq!(settings.cols, 132);
        assert_eq!(settings.rows, 42);
        assert_eq!(settings.scrollback_lines, 2048);
        assert_eq!(settings.font_family, "Cascadia Mono");
        assert_eq!(settings.font_size, 16.0);
        assert_eq!(
            settings.background_image_path.as_deref(),
            Some("C:/images/tokyo.png")
        );
        assert!((settings.background_image_opacity - 0.42).abs() < f32::EPSILON);
        assert!(!settings.cursor_blink);
        assert!(settings.copy_on_select);
        assert!(settings.gpu_acceleration);
        assert_eq!(settings.fg_color, config.active_theme().fg);
        assert_eq!(settings.bg_color, config.active_theme().bg);
        assert!(
            settings
                .global_hotkeys
                .iter()
                .any(|shortcut| shortcut == "Ctrl+Shift+D")
        );
        assert!(
            settings
                .global_hotkeys
                .iter()
                .any(|shortcut| shortcut == "Ctrl+Comma")
        );
        assert!(Arc::ptr_eq(&settings.input_suppressed, &input_suppressed));
    }

    #[test]
    fn collect_global_hotkeys_keeps_configured_and_fixed_shortcuts_unique() {
        let mut config = AppConfig::default();
        config.keyboard.split_right = "Ctrl+Shift+D".into();
        config.keyboard.open_settings = "Ctrl+Shift+V".into();

        let shortcuts = collect_global_hotkeys(&config);

        assert!(shortcuts.iter().any(|shortcut| shortcut == "Ctrl+Shift+D"));
        assert!(shortcuts.iter().any(|shortcut| shortcut == "Ctrl+Shift+V"));
        assert_eq!(
            shortcuts
                .iter()
                .filter(|shortcut| shortcut.as_str() == "Ctrl+Shift+V")
                .count(),
            1
        );
    }

    #[test]
    fn configured_global_shortcut_action_maps_ctrl_shift_s_to_split_down() {
        let config = AppConfig::default();
        let event = KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers {
                    control: true,
                    shift: true,
                    ..Modifiers::default()
                },
                key: "s".into(),
                key_char: Some("S".into()),
            },
            is_held: false,
        };

        assert_eq!(
            configured_global_shortcut_action(&event, &config),
            Some(GlobalShortcutAction::SplitDown)
        );
    }

    #[test]
    fn hotkey_binding_string_normalizes_configured_shortcuts() {
        assert_eq!(
            hotkey_binding_string("Ctrl+Shift+T").as_deref(),
            Some("ctrl-shift-t")
        );
        assert_eq!(
            hotkey_binding_string("Ctrl+Comma").as_deref(),
            Some("ctrl-comma")
        );
        assert_eq!(hotkey_binding_string("T"), None);
    }

    #[test]
    fn hotkey_string_for_keystroke_formats_special_keys() {
        let keystroke = Keystroke {
            modifiers: Modifiers {
                control: true,
                shift: true,
                ..Modifiers::default()
            },
            key: "tab".into(),
            key_char: None,
        };
        assert_eq!(
            hotkey_string_for_keystroke(&keystroke).as_deref(),
            Some("Ctrl+Shift+Tab")
        );

        let comma = Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            key: ",".into(),
            key_char: Some(",".into()),
        };
        assert_eq!(
            hotkey_string_for_keystroke(&comma).as_deref(),
            Some("Ctrl+Comma")
        );
    }

    #[test]
    fn hotkey_matches_supports_alt_and_normalized_keys() {
        let event = KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers {
                    control: true,
                    alt: true,
                    ..Modifiers::default()
                },
                key: ",".into(),
                key_char: Some(",".into()),
            },
            is_held: false,
        };

        assert!(hotkey_matches(&event, "Ctrl+Alt+Comma"));
        assert!(!hotkey_matches(&event, "Ctrl+Comma"));
    }

    fn temp_snippet_store_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("zwg-snippet-group-editor-{unique}.json"))
    }
}
