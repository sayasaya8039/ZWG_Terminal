//! Application state and root view — Figma-aligned macOS terminal chrome

use std::io::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

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
    default_model_for_provider, next_ai_provider, resolve_ai_api_key, resolve_ai_model,
    sanitize_ai_provider,
};
use crate::config::{
    AppConfig, DEFAULT_TERMINAL_FONT_FAMILY, DEFAULT_UI_FONT_FAMILY,
    SUPPORTED_TERMINAL_FONT_FAMILIES, WindowState, set_launch_on_login,
};
use crate::shell::{self, ShellType};
use crate::snippet_palette::{SnippetPaletteModel, SnippetSection};
use crate::split::{FocusDir, SplitContainer, SplitDirection};
use crate::terminal::TerminalSettings;
use crate::terminal::view::{CELL_HEIGHT_ESTIMATE, CELL_WIDTH_ESTIMATE, WINDOW_CHROME_HEIGHT};
use crate::{
    ClosePane, CloseTab, FocusNext, FocusPrev, NewTab, OpenSettings, Quit, SplitDown, SplitRight,
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
static INPUT_METHOD_VK_PROCESSKEY: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
unsafe extern "system" fn input_method_getmessage_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, MSG, PM_REMOVE, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION,
        WM_IME_STARTCOMPOSITION, WM_KEYDOWN,
    };

    if code >= 0 && wparam.0 == PM_REMOVE.0 as usize {
        unsafe {
            let msg = &*(lparam.0 as *const MSG);
            if input_method_trace_enabled() {
                match msg.message {
                    message if message == WM_IME_STARTCOMPOSITION => {
                        log::debug!(
                            "IME_CMP_START time={} wparam=0x{:X} lparam=0x{:X}",
                            msg.time,
                            msg.wParam.0,
                            msg.lParam.0
                        );
                    }
                    message if message == WM_IME_COMPOSITION => {
                        log::debug!(
                            "IME_CMP message time={} wparam=0x{:X} lparam=0x{:X}",
                            msg.time,
                            msg.wParam.0,
                            msg.lParam.0
                        );
                    }
                    message if message == WM_IME_ENDCOMPOSITION => {
                        log::debug!(
                            "IME_CMP_END time={} wparam=0x{:X} lparam=0x{:X}",
                            msg.time,
                            msg.wParam.0,
                            msg.lParam.0
                        );
                    }
                    _ => {}
                }
            }
            if msg.message == WM_KEYDOWN {
                let vk = (msg.wParam.0 & 0xFFFF) as u16;
                if input_method_trace_enabled() {
                    log::debug!(
                        "IME_HOOK raw keydown vk=0x{:04X} code={} wparam=0x{:X} lparam=0x{:X}",
                        vk,
                        msg.message,
                        msg.wParam.0,
                        msg.lParam.0,
                    );
                }
                if vk == 0xE5 {
                    if input_method_trace_enabled() {
                        log::debug!(
                            "IME_HOOK VK_PROCESSKEY detected -> latch (flag only, no TranslateMessage)"
                        );
                    }
                    // NOTE: TranslateMessage is called by the terminal IME hook
                    // (terminal/view.rs) only.  Calling it from both hooks causes
                    // duplicate WM_IME_COMPOSITION messages that confuse the IME.
                    INPUT_METHOD_VK_PROCESSKEY.store(true, Ordering::Release);
                }
            }
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(target_os = "windows")]
fn install_input_method_hook() {
    use std::sync::Once;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_GETMESSAGE};

    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let thread_id = GetCurrentThreadId();
        match SetWindowsHookExW(
            WH_GETMESSAGE,
            Some(input_method_getmessage_hook_proc),
            None,
            thread_id,
        ) {
            Ok(_) => log::info!("Input method GetMessage hook installed"),
            Err(e) => log::error!("Failed to install input method hook: {}", e),
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn install_input_method_hook() {}

fn input_method_trace_enabled() -> bool {
    use std::sync::OnceLock;

    static TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
    *TRACE_ENABLED.get_or_init(|| {
        std::env::var("ZWG_IME_TRACE")
            .map(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                )
            })
            .unwrap_or(false)
    })
}

fn log_input_method_keystroke(context: &str, keystroke: &Keystroke, detail: &str) {
    if !input_method_trace_enabled() {
        return;
    }

    log::debug!(
        "IME_TRACE [{}] key={} key_char={:?} ctrl:{} alt:{} shift:{} detail={}",
        context,
        keystroke.key,
        keystroke.key_char,
        keystroke.modifiers.control,
        keystroke.modifiers.alt,
        keystroke.modifiers.shift,
        detail
    );
}

fn log_input_method_text(context: &str, target: Option<AiSettingsImeTarget>, text: &str) {
    if !input_method_trace_enabled() {
        return;
    }

    log::debug!("IME_TEXT [{}] target={:?} text={:?}", context, target, text);
}

fn should_defer_keystroke_to_input_method(keystroke: &Keystroke) -> bool {
    let ime_processkey_pending = INPUT_METHOD_VK_PROCESSKEY.swap(false, Ordering::AcqRel);
    if !ime_processkey_pending {
        log_input_method_keystroke(
            "should_defer_keystroke_to_input_method",
            keystroke,
            "processkey not pending -> direct",
        );
        return false;
    }

    if let Some(text) = direct_text_from_input_keystroke(keystroke) {
        // Keep direct IME-resolved non-ASCII characters (e.g. あ, 漢字候補確定) in snippet inputs,
        // but continue deferring ASCII keystrokes during IME processing.
        let defer = text.chars().all(|ch| ch.is_ascii());
        log_input_method_keystroke(
            "should_defer_keystroke_to_input_method",
            keystroke,
            &format!("direct_text={:?}, defer={}", text, defer),
        );
        return defer;
    }

    let defer = !keystroke
        .key_char
        .as_ref()
        .is_some_and(|key_char| !key_char.is_empty());
    log_input_method_keystroke(
        "should_defer_keystroke_to_input_method",
        keystroke,
        &format!("fallback key_char defer={}", defer),
    );
    defer
}

fn direct_text_from_input_keystroke(keystroke: &Keystroke) -> Option<String> {
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
const SNIPPET_PANEL_MARGIN: f32 = 12.0;
const SNIPPET_PANEL_TOP_OFFSET: f32 = 46.0;
const SNIPPET_PANEL_MAX_WIDTH: f32 = 940.0;
const SNIPPET_PANEL_MAX_HEIGHT: f32 = 560.0;

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
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AiSettingsTextField {
    ApiKey,
    Model,
}

#[derive(Clone)]
struct AppNotice {
    title: String,
    detail: String,
    created_at: Instant,
    duration_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SnippetPanelFrame {
    top: f32,
    left: f32,
    width: f32,
    height: f32,
}

fn snippet_panel_frame(viewport_w: f32, viewport_h: f32) -> SnippetPanelFrame {
    let width = (viewport_w - (SNIPPET_PANEL_MARGIN * 2.0))
        .max(420.0)
        .min(SNIPPET_PANEL_MAX_WIDTH);
    let height = (viewport_h - SNIPPET_PANEL_TOP_OFFSET - 16.0)
        .max(260.0)
        .min(SNIPPET_PANEL_MAX_HEIGHT);
    let left = (viewport_w - width - SNIPPET_PANEL_MARGIN).max(SNIPPET_PANEL_MARGIN);

    SnippetPanelFrame {
        top: SNIPPET_PANEL_TOP_OFFSET,
        left,
        width,
        height,
    }
}

#[cfg(test)]
fn next_filtered_index(current: Option<usize>, visible: &[usize], step: isize) -> Option<usize> {
    if visible.is_empty() {
        return None;
    }

    let current_position = current
        .and_then(|value| visible.iter().position(|candidate| *candidate == value))
        .unwrap_or(0);
    let next_position = (current_position as isize + step).rem_euclid(visible.len() as isize);
    visible.get(next_position as usize).copied()
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

fn active_ai_settings_ime_target(
    active_field: Option<AiSettingsTextField>,
) -> Option<AiSettingsImeTarget> {
    match active_field {
        Some(AiSettingsTextField::ApiKey) => Some(AiSettingsImeTarget::ApiKey),
        Some(AiSettingsTextField::Model) => Some(AiSettingsImeTarget::Model),
        None => None,
    }
}

fn current_text_for_ai_settings_ime_target(
    config: &AppConfig,
    target: AiSettingsImeTarget,
) -> Option<String> {
    match target {
        AiSettingsImeTarget::ApiKey => Some(config.ai_api_key.clone()),
        AiSettingsImeTarget::Model => Some(config.ai_model.clone()),
    }
}

fn replace_text_in_ai_settings_ime_target(
    config: &mut AppConfig,
    target: AiSettingsImeTarget,
    range: Range<usize>,
    text: &str,
) -> Option<Range<usize>> {
    let value = match target {
        AiSettingsImeTarget::ApiKey => &mut config.ai_api_key,
        AiSettingsImeTarget::Model => &mut config.ai_model,
    };

    let range = range.start.min(value.len())..range.end.min(value.len());
    value.replace_range(range.clone(), text);
    Some(range.start..range.start + text.len())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiSettingsImeTarget {
    ApiKey,
    Model,
}

/// Root view containing tab bar + split container + overlays.
pub struct RootView {
    state: Entity<AppState>,
    focus_handle: FocusHandle,
    show_shell_menu: bool,
    show_settings: bool,
    show_snippet_palette: bool,
    show_close_confirm: bool,
    snippet_palette: SnippetPaletteModel,
    keyboard_settings_active_text: Option<KeyboardSettingsTextField>,
    ai_settings_active_text: Option<AiSettingsTextField>,
    app_notice: Option<AppNotice>,
    ai_settings_ime_target: Option<AiSettingsImeTarget>,
    ai_settings_ime_marked_range: Option<Range<usize>>,
    ai_settings_ime_selected_range: Option<Range<usize>>,
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
        install_input_method_hook();

        Self {
            state,
            focus_handle: _cx.focus_handle(),
            show_shell_menu: false,
            show_settings: false,
            show_snippet_palette: false,
            show_close_confirm: false,
            snippet_palette: SnippetPaletteModel::new(),
            keyboard_settings_active_text: None,
            ai_settings_active_text: None,
            app_notice: None,
            ai_settings_ime_target: None,
            ai_settings_ime_marked_range: None,
            ai_settings_ime_selected_range: None,
            settings_category: SettingsCategory::General,
            last_bounds: None,
            last_save_time: Instant::now(),
            bounds_dirty: false,
        }
    }

    fn compute_ai_settings_ime_target(&self) -> Option<AiSettingsImeTarget> {
        if self.show_settings {
            if let Some(target) = active_ai_settings_ime_target(self.ai_settings_active_text) {
                return Some(target);
            }
        }

        None
    }

    fn sync_ai_settings_ime_target(&mut self) {
        let target = self.compute_ai_settings_ime_target();
        if self.ai_settings_ime_target != target {
            self.ai_settings_ime_target = target;
            self.ai_settings_ime_marked_range = None;
            self.ai_settings_ime_selected_range = None;
        }
    }

    fn clear_ai_settings_ime_state(&mut self) {
        self.ai_settings_ime_marked_range = None;
        self.ai_settings_ime_selected_range = None;
    }

    fn current_ai_settings_ime_text(
        &self,
        target: AiSettingsImeTarget,
        cx: &Context<Self>,
    ) -> Option<String> {
        match target {
            AiSettingsImeTarget::ApiKey | AiSettingsImeTarget::Model => {
                current_text_for_ai_settings_ime_target(&self.state.read(cx).config, target)
            }
        }
    }

    fn current_ai_settings_ime_selection_range(
        &self,
        target: AiSettingsImeTarget,
        cx: &Context<Self>,
    ) -> Option<Range<usize>> {
        if let Some(range) = self.ai_settings_ime_selected_range.clone() {
            return Some(range);
        }

        match target {
            AiSettingsImeTarget::ApiKey | AiSettingsImeTarget::Model => {
                current_text_for_ai_settings_ime_target(&self.state.read(cx).config, target).map(
                    |text| {
                        let len = text.len();
                        len..len
                    },
                )
            }
        }
    }

    fn replace_text_in_active_ai_settings_ime_target(
        &mut self,
        target: AiSettingsImeTarget,
        range: Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        match target {
            AiSettingsImeTarget::ApiKey | AiSettingsImeTarget::Model => {
                let mut config = self.state.read(cx).config.clone();
                let inserted =
                    replace_text_in_ai_settings_ime_target(&mut config, target, range, text)?;
                self.persist_config_update(cx, move |app_config| {
                    app_config.ai_api_key = config.ai_api_key;
                    app_config.ai_model = config.ai_model;
                });
                Some(inserted)
            }
        }
    }

    fn open_settings_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_shell_menu = false;
        self.show_snippet_palette = false;
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

    fn on_open_settings(
        &mut self,
        _action: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_panel(window, cx);
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
            self.show_snippet_palette = false;
            self.show_settings = false;
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
    }

    fn shortcut_actions_blocked(&self) -> bool {
        self.show_settings || self.show_close_confirm || self.show_snippet_palette
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
            || self.show_close_confirm
            || self.show_snippet_palette;
        self.state
            .read(cx)
            .terminal_input_suppressed
            .store(suppressed, Ordering::Relaxed);
    }

    fn toggle_snippet_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next_visible = !self.show_snippet_palette;
        self.show_snippet_palette = next_visible;
        self.show_shell_menu = false;
        self.show_settings = false;
        if next_visible {
            window.focus(&self.focus_handle);
        }
        cx.notify();
    }

    fn close_snippet_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_snippet_palette {
            return;
        }
        self.show_snippet_palette = false;
        cx.notify();
        self.focus_active_terminal(window, cx);
    }

    fn move_snippet_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        if self.snippet_palette.move_selection(step) {
            cx.notify();
        }
    }

    fn copy_selected_snippet(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.snippet_palette.selected_snippet() else {
            return;
        };
        let section_label = self.snippet_palette.active_section().title();
        cx.write_to_clipboard(ClipboardItem::new_string(item.content.clone()));
        self.show_app_notice(
            format!("{section_label}をコピーしました"),
            format!("{} をクリップボードへ送信しました。", item.title),
            2200,
            cx,
        );
    }

    fn select_snippet(&mut self, snippet_id: &str, cx: &mut Context<Self>) {
        if self.snippet_palette.select(snippet_id) {
            cx.notify();
        }
    }

    fn select_snippet_section(&mut self, section: SnippetSection, cx: &mut Context<Self>) {
        if self.snippet_palette.select_section(section) {
            cx.notify();
        }
    }

    fn create_new_snippet_item(&mut self, cx: &mut Context<Self>) {
        let section_label = self.snippet_palette.active_section().title().to_string();
        if self.snippet_palette.create_new_item().is_some() {
            self.show_app_notice(
                format!("{section_label}に新規項目を追加しました"),
                "追加した項目を選択しています。".to_string(),
                1800,
                cx,
            );
            cx.notify();
        }
    }

    fn cycle_snippet_sections(&mut self, step: isize, cx: &mut Context<Self>) {
        if self.snippet_palette.cycle_sections(step) {
            cx.notify();
        }
    }

    fn append_snippet_search_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.snippet_palette.append_search_query(text);
        cx.notify();
    }

    fn paste_into_snippet_search(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.snippet_palette.append_search_query(&text);
        cx.notify();
    }

    fn pop_snippet_search_text(&mut self, cx: &mut Context<Self>) {
        if self.snippet_palette.pop_search_query() {
            cx.notify();
        }
    }

    fn clear_snippet_search_text(&mut self, cx: &mut Context<Self>) {
        if self.snippet_palette.clear_search_query() {
            cx.notify();
        }
    }

    fn toggle_selected_snippet_pin(&mut self, cx: &mut Context<Self>) {
        let Some(title) = self
            .snippet_palette
            .selected_snippet()
            .map(|item| item.title.clone())
        else {
            return;
        };
        let Some(pinned) = self.snippet_palette.toggle_selected_pinned() else {
            return;
        };
        self.show_app_notice(
            if pinned {
                "項目をピン留めしました"
            } else {
                "ピン留めを解除しました"
            },
            format!("{title} の優先表示状態を更新しました。"),
            1800,
            cx,
        );
        cx.notify();
    }

    fn toggle_snippet_pinned_only(&mut self, cx: &mut Context<Self>) {
        self.snippet_palette.toggle_pinned_only();
        cx.notify();
    }

    fn delete_selected_snippet(&mut self, cx: &mut Context<Self>) {
        let section_label = self.snippet_palette.active_section().title().to_string();
        let Some(title) = self
            .snippet_palette
            .selected_snippet()
            .map(|item| item.title.clone())
        else {
            return;
        };
        if self.snippet_palette.remove_selected() {
            self.show_app_notice(
                format!("{section_label}を削除しました"),
                format!("{title} を現在の一覧から取り除きました。"),
                1800,
                cx,
            );
            cx.notify();
        }
    }

    fn handle_snippet_palette_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.show_snippet_palette {
            return false;
        }

        if should_defer_keystroke_to_input_method(&event.keystroke) {
            return true;
        }

        match event.keystroke.key.as_ref() {
            "escape" => {
                self.close_snippet_palette(window, cx);
                true
            }
            "up" => {
                self.move_snippet_selection(-1, cx);
                true
            }
            "down" => {
                self.move_snippet_selection(1, cx);
                true
            }
            "tab" => {
                self.cycle_snippet_sections(
                    if event.keystroke.modifiers.shift {
                        -1
                    } else {
                        1
                    },
                    cx,
                );
                true
            }
            "left" if !event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                self.cycle_snippet_sections(-1, cx);
                true
            }
            "right" if !event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                self.cycle_snippet_sections(1, cx);
                true
            }
            "enter" => {
                self.copy_selected_snippet(cx);
                true
            }
            "backspace" => {
                self.pop_snippet_search_text(cx);
                true
            }
            "delete" => {
                self.delete_selected_snippet(cx);
                true
            }
            "v" if event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                self.paste_into_snippet_search(cx);
                true
            }
            "insert"
                if event.keystroke.modifiers.shift
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt =>
            {
                self.paste_into_snippet_search(cx);
                true
            }
            "l" if event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                self.clear_snippet_search_text(cx);
                true
            }
            "p" if event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                self.toggle_selected_snippet_pin(cx);
                true
            }
            "f" if event.keystroke.modifiers.control && !event.keystroke.modifiers.alt => {
                self.toggle_snippet_pinned_only(cx);
                true
            }
            _ => {
                if let Some(text) = direct_text_from_input_keystroke(&event.keystroke) {
                    self.append_snippet_search_text(&text, cx);
                    true
                } else {
                    false
                }
            }
        }
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

        if should_defer_keystroke_to_input_method(&event.keystroke) {
            return true;
        }

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

        if let Some(text) = direct_text_from_input_keystroke(&event.keystroke) {
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

    fn handle_custom_ai_settings_hotkeys(
        &mut self,
        _event: &KeyDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
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
            GlobalShortcutAction::Quit => self.on_quit_requested(&Quit, window, cx),
        }

        true
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ai_settings_ime_target = self.compute_ai_settings_ime_target();
        log_input_method_keystroke(
            "on_key_down",
            &event.keystroke,
            &format!("ai_settings_ime_target={:?}", ai_settings_ime_target),
        );

        if self.handle_ai_settings_key(event, window, cx)
            || self.handle_keyboard_settings_key(event, window, cx)
        {
            cx.stop_propagation();
            return;
        }

        if ai_settings_ime_target.is_some() {
            cx.stop_propagation();
            return;
        }

        if self.handle_custom_ai_settings_hotkeys(event, window, cx) {
            cx.stop_propagation();
            return;
        }

        if self.handle_snippet_palette_key(event, window, cx) {
            cx.stop_propagation();
            return;
        }

        if self.handle_global_shortcut_key(event, window, cx) {
            cx.stop_propagation();
            return;
        }
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

    fn render_snippet_palette(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_snippet_palette {
            return None;
        }

        let frame = snippet_panel_frame(viewport_w, viewport_h);
        let sidebar_w = (frame.width * 0.36).clamp(276.0, 332.0);
        let visible = self.snippet_palette.visible_snippets();
        let selected_id = self
            .snippet_palette
            .selected_snippet()
            .map(|snippet| snippet.id.clone());
        let search_query = self.snippet_palette.search_query().to_string();
        let pinned_only = self.snippet_palette.pinned_only();
        let total_count = self.snippet_palette.total_count();
        let pinned_count = self.snippet_palette.pinned_count();
        let active_section = self.snippet_palette.active_section();
        let active_section_label = active_section.title().to_string();

        let list_items = if visible.is_empty() {
            vec![
                div()
                    .w_full()
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(rgba(0xffffff10))
                    .bg(rgba(0xffffff06))
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .font_family(UI_FONT)
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT))
                            .child(active_section.empty_label()),
                    )
                    .child(
                        div()
                            .font_family(UI_FONT)
                            .text_size(px(11.0))
                            .text_color(rgb(SUBTEXT1))
                            .child(if search_query.is_empty() {
                                "フィルタを解除するか、履歴と定型文を切り替えてください。"
                            } else {
                                "検索語を短くするか、Ctrl+L で検索をクリアしてください。"
                            }),
                    )
                    .into_any_element(),
            ]
        } else {
            visible
                .iter()
                .map(|item| {
                    let item_id = item.id.clone();
                    let is_selected = selected_id.as_deref() == Some(item.id.as_str());
                    let relative_created_label = item.relative_created_label();
                    let meta_row = if active_section == SnippetSection::History {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                svg()
                                    .path("ui/clock.svg")
                                    .size(px(11.0))
                                    .text_color(rgb(if is_selected { 0xffffff } else { MUTED })),
                            )
                            .children(relative_created_label.clone().into_iter().map(|label| {
                                div()
                                    .font_family(UI_FONT)
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(if is_selected { 0xffffff } else { SUBTEXT0 }))
                                    .child(label)
                                    .into_any_element()
                            }))
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_size(px(10.0))
                                    .text_color(rgb(if is_selected { 0xffffff } else { MUTED }))
                                    .child("•"),
                            )
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_size(px(10.0))
                                    .text_color(rgb(if is_selected { 0xffffff } else { MUTED }))
                                    .child(item.source.clone()),
                            )
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    };

                    let mut row = div()
                        .id(ElementId::Name(format!("snippet-item-{}", item.id).into()))
                        .w_full()
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(if is_selected {
                            rgba(0x3B82F6FF)
                        } else {
                            rgba(0xffffff00)
                        })
                        .bg(if is_selected {
                            rgba(0x2563EBFF)
                        } else {
                            rgba(0xffffff00)
                        })
                        .px(px(12.0))
                        .py(px(10.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(0xffffff10)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                this.select_snippet(&item_id, cx);
                            }),
                        )
                        .flex()
                        .gap(px(10.0))
                        .child(
                            div()
                                .w(px(28.0))
                                .h(px(28.0))
                                .rounded(px(8.0))
                                .bg(if is_selected {
                                    rgba(0xffffff1F)
                                } else {
                                    rgba(0xffffff10)
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    svg()
                                        .path("ui/snippet-palette.svg")
                                        .size(px(14.0))
                                        .text_color(rgb(if is_selected {
                                            0xffffff
                                        } else {
                                            TEXT_SOFT
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(if is_selected {
                                                    0xffffff
                                                } else {
                                                    TEXT
                                                }))
                                                .child(item.title.clone()),
                                        )
                                        .child(if item.pinned {
                                            svg()
                                                .path("ui/star-filled.svg")
                                                .size(px(12.0))
                                                .text_color(rgb(0xF5C542))
                                                .into_any_element()
                                        } else {
                                            div().into_any_element()
                                        }),
                                )
                                .child(if item.summary.is_empty() {
                                    div().into_any_element()
                                } else {
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(11.0))
                                        .text_color(rgb(if is_selected {
                                            0xDBEAFE
                                        } else {
                                            SUBTEXT1
                                        }))
                                        .child(item.summary.clone())
                                        .into_any_element()
                                })
                                .child(meta_row),
                        );

                    if is_selected {
                        row = row.shadow_lg();
                    }

                    row.into_any_element()
                })
                .collect::<Vec<_>>()
        };

        let detail = if let Some(item) = self.snippet_palette.selected_snippet().cloned() {
            let detail_meta = format!("{} • {}", item.kind_label, item.source);
            let content_lines = item
                .content
                .lines()
                .map(|line| {
                    div()
                        .font_family(MONO_FONT)
                        .text_size(px(12.0))
                        .text_color(rgb(TEXT))
                        .child(if line.is_empty() {
                            " ".to_string()
                        } else {
                            line.to_string()
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>();

            div()
                .flex_1()
                .min_w(px(0.0))
                .bg(rgb(PANEL_BG))
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(58.0))
                        .px(px(18.0))
                        .border_b_1()
                        .border_color(rgba(0xffffff10))
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(SUBTEXT0))
                                        .child(detail_meta),
                                ),
                        )
                        .child(
                            panel_icon_button("snippet-detail-pin", "ui/star.svg", item.pinned)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                        this.toggle_selected_snippet_pin(cx);
                                    }),
                                ),
                        )
                        .child(
                            panel_icon_button("snippet-detail-copy", "ui/copy.svg", false)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                        this.copy_selected_snippet(cx);
                                    }),
                                ),
                        )
                        .child(
                            panel_icon_button("snippet-detail-delete", "ui/trash.svg", false)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                        this.delete_selected_snippet(cx);
                                    }),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("snippet-detail-scroll")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_scroll()
                        .scrollbar_width(px(6.0))
                        .child(
                            div()
                                .w_full()
                                .px(px(18.0))
                                .py(px(16.0))
                                .rounded(px(12.0))
                                .border_1()
                                .border_color(rgba(0xffffff10))
                                .bg(rgba(0xffffff06))
                                .p(px(16.0))
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .children(content_lines),
                        ),
                )
                .child(
                    div()
                        .h(px(42.0))
                        .px(px(18.0))
                        .border_t_1()
                        .border_color(rgba(0xffffff10))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(px(11.0))
                                .text_color(rgb(SUBTEXT1))
                                .child(item.created_label.clone()),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .font_family(UI_FONT)
                        .text_size(px(13.0))
                        .text_color(rgb(SUBTEXT1))
                        .child(active_section.empty_label()),
                )
                .into_any_element()
        };

        Some(
            div()
                .id("snippet-panel")
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.close_snippet_palette(window, cx);
                }))
                .absolute()
                .top(px(frame.top))
                .left(px(frame.left))
                .w(px(frame.width))
                .h(px(frame.height))
                .rounded(px(14.0))
                .overflow_hidden()
                .border_1()
                .border_color(rgba(0xffffff18))
                .bg(rgb(PANEL_BG))
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(52.0))
                        .px(px(16.0))
                        .border_b_1()
                        .border_color(rgba(0xffffff10))
                        .bg(linear_gradient(
                            180.0,
                            linear_color_stop(rgba(0x3B3B3DEB), 0.0),
                            linear_color_stop(rgba(0x2F2F31F8), 1.0),
                        ))
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .w(px(30.0))
                                .h(px(30.0))
                                .rounded(px(9.0))
                                .bg(rgba(0xffffff12))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    svg()
                                        .path("ui/snippet-palette.svg")
                                        .size(px(15.0))
                                        .text_color(rgb(TEXT_SOFT)),
                                ),
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
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(TEXT))
                                        .child("クリップボードマネージャー"),
                                )
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(px(11.0))
                                        .text_color(rgb(SUBTEXT1))
                                        .child(format!(
                                            "{} {} 件 / ピン留め {} 件",
                                            active_section_label, total_count, pinned_count
                                        )),
                                ),
                        )
                        .child(
                            panel_text_button("snippet-panel-new", "ui/plus.svg", "新規")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                                        this.create_new_snippet_item(cx);
                                    }),
                                ),
                        )
                        .child(
                            panel_icon_button("snippet-panel-settings", "ui/settings.svg", false)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                        this.open_settings_panel(window, cx);
                                    }),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .flex()
                        .child(
                            div()
                                .w(px(sidebar_w))
                                .min_w(px(sidebar_w))
                                .bg(rgb(PANEL_SIDEBAR_BG))
                                .border_r_1()
                                .border_color(rgba(0xffffff10))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .p(px(16.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(12.0))
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(MUTED))
                                                .child("検索"),
                                        )
                                        .child(
                                            div()
                                                .h(px(34.0))
                                                .rounded(px(10.0))
                                                .bg(rgba(0xffffff0E))
                                                .border_1()
                                                .border_color(rgba(0xffffff10))
                                                .px(px(12.0))
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .child(
                                                    svg()
                                                        .path("ui/search.svg")
                                                        .size(px(13.0))
                                                        .text_color(rgb(SUBTEXT1)),
                                                )
                                                .child(
                                                    div()
                                                        .font_family(UI_FONT)
                                                        .text_size(px(12.0))
                                                        .text_color(if search_query.is_empty() {
                                                            rgb(SUBTEXT1)
                                                        } else {
                                                            rgb(TEXT)
                                                        })
                                                        .child(if search_query.is_empty() {
                                                            "検索".to_string()
                                                        } else {
                                                            search_query.clone()
                                                        }),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .gap(px(8.0))
                                                .children(
                                                    [SnippetSection::History, SnippetSection::Template]
                                                        .into_iter()
                                                        .map(|section| {
                                                            let active = active_section == section;
                                                            let section_value = section;

                                                            div()
                                                                .id(ElementId::Name(
                                                                    format!(
                                                                        "snippet-section-{}",
                                                                        section.title()
                                                                    )
                                                                    .into(),
                                                                ))
                                                                .rounded(px(11.0))
                                                                .border_1()
                                                                .border_color(if active {
                                                                    rgba(0x0A84FF99)
                                                                } else {
                                                                    rgba(0xffffff10)
                                                                })
                                                                .bg(if active {
                                                                    rgba(0x0A84FF2C)
                                                                } else {
                                                                    rgba(0xffffff08)
                                                                })
                                                                .px(px(12.0))
                                                                .py(px(8.0))
                                                                .cursor_pointer()
                                                                .hover(|style| {
                                                                    style.bg(rgba(0xffffff14))
                                                                })
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    cx.listener(
                                                                        move |this,
                                                                              _: &MouseDownEvent,
                                                                              _window,
                                                                              cx| {
                                                                            this.select_snippet_section(
                                                                                section_value,
                                                                                cx,
                                                                            );
                                                                        },
                                                                    ),
                                                                )
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .child(
                                                                    div()
                                                                        .font_family(UI_FONT)
                                                                        .text_size(px(12.0))
                                                                        .font_weight(
                                                                            FontWeight::MEDIUM,
                                                                        )
                                                                        .text_color(rgb(if active {
                                                                            TEXT
                                                                        } else {
                                                                            SUBTEXT0
                                                                        }))
                                                                        .child(section.title()),
                                                                )
                                                                .into_any_element()
                                                        }),
                                                )
                                        )
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(11.0))
                                                .text_color(rgb(MUTED))
                                                .child(if pinned_only {
                                                    format!("{active_section_label}のピン留めのみ表示中")
                                                } else {
                                                    "Tab・←→ で履歴と定型文を切り替えます。".to_string()
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("snippet-list-scroll")
                                        .flex_1()
                                        .min_h(px(0.0))
                                        .overflow_scroll()
                                        .scrollbar_width(px(6.0))
                                        .child(
                                            div()
                                                .px(px(10.0))
                                                .pb(px(10.0))
                                                .flex()
                                                .flex_col()
                                                .gap(px(6.0))
                                                .children(list_items),
                                        ),
                                )
                                .child(
                                    div()
                                        .h(px(40.0))
                                        .px(px(16.0))
                                        .border_t_1()
                                        .border_color(rgba(0xffffff10))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(11.0))
                                                .text_color(rgb(SUBTEXT1))
                                                .child(format!("表示中 {} 件", visible.len())),
                                        )
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_size(px(11.0))
                                                .text_color(rgb(MUTED))
                                                .child(if search_query.is_empty() {
                                                    "アンカード表示"
                                                } else {
                                                    "Ctrl+L で検索クリア"
                                                }),
                                        ),
                                ),
                        )
                        .child(detail),
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
        let ime_entity = cx.entity();
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
                div()
                    .relative()
                    .child(interactive_input_box_dynamic(
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
                    ))
                    .when(
                        self.ai_settings_ime_target == Some(AiSettingsImeTarget::ApiKey),
                        |node| {
                            node.child(ai_settings_ime_input_overlay(
                                ime_entity.clone(),
                                self.focus_handle.clone(),
                            ))
                        },
                    ),
            ))
            .child(settings_row(
                "モデル",
                div()
                    .relative()
                    .child(interactive_input_box_dynamic(
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
                    ))
                    .when(
                        self.ai_settings_ime_target == Some(AiSettingsImeTarget::Model),
                        |node| {
                            node.child(ai_settings_ime_input_overlay(
                                ime_entity.clone(),
                                self.focus_handle.clone(),
                            ))
                        },
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
        self.maybe_clear_app_notice();
        self.sync_ai_settings_ime_target();
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

        let side_cluster_width = titlebar_side_cluster_width();

        let mut titlebar_actions = div()
            .id("titlebar-actions")
            .w(px(titlebar_actions_width()))
            .min_w(px(titlebar_actions_width()))
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
                    this.show_snippet_palette = false;
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
                        this.show_snippet_palette = false;
                        this.show_shell_menu = true;
                        cx.notify();
                        cx.stop_propagation();
                    }),
                ),
            );

        titlebar_actions = titlebar_actions
            .child(
                chrome_button("title-snippets", "ui/snippet-palette.svg").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.toggle_snippet_palette(window, cx);
                        cx.stop_propagation();
                    }),
                ),
            )
            .child(
                chrome_button("title-settings", "ui/settings.svg").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
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
            .children(self.render_snippet_palette(new_state.width, new_state.height, cx))
            .children(settings_backdrop)
            .children(self.render_settings_panel(new_state.width, new_state.height, window, cx))
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
            .on_action(cx.listener(Self::on_quit_requested))
            .child(window_surface)
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
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
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let target = self.compute_ai_settings_ime_target()?;
        let text = self.current_ai_settings_ime_text(target, cx)?;
        let range = utf16_range_to_byte_range(&text, &range_utf16);
        adjusted_range.replace(byte_range_to_utf16_range(&text, &range));
        Some(text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let target = self.compute_ai_settings_ime_target()?;
        let text = self.current_ai_settings_ime_text(target, cx)?;
        let range = self.current_ai_settings_ime_selection_range(target, cx)?;
        Some(UTF16Selection {
            range: byte_range_to_utf16_range(&text, &range),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let target = self.compute_ai_settings_ime_target()?;
        let text = self.current_ai_settings_ime_text(target, cx)?;
        self.ai_settings_ime_marked_range
            .as_ref()
            .map(|range| byte_range_to_utf16_range(&text, range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.clear_ai_settings_ime_state();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.compute_ai_settings_ime_target() else {
            return;
        };
        log_input_method_text("replace_text_in_range start", Some(target), text);
        let Some(current_text) = self.current_ai_settings_ime_text(target, cx) else {
            return;
        };
        let range = range_utf16
            .as_ref()
            .map(|range| utf16_range_to_byte_range(&current_text, range))
            .or_else(|| self.ai_settings_ime_marked_range.clone())
            .or_else(|| self.current_ai_settings_ime_selection_range(target, cx))
            .unwrap_or_else(|| current_text.len()..current_text.len());
        INPUT_METHOD_VK_PROCESSKEY.store(false, Ordering::Release);
        if let Some(inserted) =
            self.replace_text_in_active_ai_settings_ime_target(target, range, text, cx)
        {
            self.ai_settings_ime_marked_range = None;
            self.ai_settings_ime_selected_range = Some(inserted.end..inserted.end);
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
        let Some(target) = self.compute_ai_settings_ime_target() else {
            return;
        };
        log_input_method_text(
            "replace_and_mark_text_in_range start",
            Some(target),
            new_text,
        );
        let Some(current_text) = self.current_ai_settings_ime_text(target, cx) else {
            return;
        };
        let range = range_utf16
            .as_ref()
            .map(|value| utf16_range_to_byte_range(&current_text, value))
            .or_else(|| self.ai_settings_ime_marked_range.clone())
            .or_else(|| self.current_ai_settings_ime_selection_range(target, cx))
            .unwrap_or_else(|| current_text.len()..current_text.len());
        INPUT_METHOD_VK_PROCESSKEY.store(false, Ordering::Release);
        if let Some(inserted) =
            self.replace_text_in_active_ai_settings_ime_target(target, range.clone(), new_text, cx)
        {
            self.ai_settings_ime_marked_range = if new_text.is_empty() {
                None
            } else {
                Some(inserted.clone())
            };
            self.ai_settings_ime_selected_range = new_selected_range_utf16
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

fn ai_settings_ime_input_overlay(
    entity: Entity<RootView>,
    focus_handle: FocusHandle,
) -> AnyElement {
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

fn panel_icon_button(id: &'static str, icon_path: &'static str, active: bool) -> Stateful<Div> {
    let mut button = chrome_button(id, icon_path)
        .w(px(28.0))
        .min_w(px(28.0))
        .h(px(28.0))
        .rounded(px(8.0))
        .bg(rgba(if active { 0x0A84FF33 } else { 0xffffff00 }));

    if active {
        button = button.border_1().border_color(rgba(0x0A84FF66));
    }

    button
}

fn panel_text_button(
    id: &'static str,
    icon_path: &'static str,
    label: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.0))
        .rounded(px(8.0))
        .px(px(10.0))
        .bg(rgba(0xffffff10))
        .border_1()
        .border_color(rgba(0xffffff12))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(6.0))
        .hover(|style| style.bg(rgba(0xffffff18)))
        .child(
            svg()
                .path(icon_path)
                .size(px(12.0))
                .text_color(rgb(TEXT_SOFT)),
        )
        .child(
            div()
                .font_family(UI_FONT)
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT))
                .child(label),
        )
}

fn titlebar_actions_width() -> f32 {
    let action_slots: usize = 4;
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

fn titlebar_side_cluster_width() -> f32 {
    titlebar_actions_width().max(traffic_lights_width())
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
        AiSettingsImeTarget, AiSettingsTextField, GlobalShortcutAction, INPUT_METHOD_VK_PROCESSKEY,
        ZoomAction, active_ai_settings_ime_target, adjust_font_size_value,
        byte_index_to_utf16_offset, byte_range_to_utf16_range, collect_global_hotkeys,
        configured_global_shortcut_action, current_text_for_ai_settings_ime_target,
        cycle_string_option, direct_text_from_input_keystroke, hotkey_binding_string,
        hotkey_matches, hotkey_string_for_keystroke, next_filtered_index,
        process_completion_notice_detail, replace_text_in_ai_settings_ime_target,
        should_defer_keystroke_to_input_method, snippet_panel_frame, terminal_settings_from_config,
        titlebar_actions_width, titlebar_side_cluster_width, traffic_lights_width,
        utf16_offset_to_byte_index, utf16_range_to_byte_range, zoom_action_for_window,
    };
    use crate::config::AppConfig;
    use gpui::{KeyDownEvent, Keystroke, Modifiers};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

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
    fn titlebar_actions_width_matches_four_button_layout() {
        // 4 buttons × 24px + 3 gaps × 4px = 108px
        assert_eq!(titlebar_actions_width(), 108.0);
    }

    #[test]
    fn titlebar_side_cluster_width_tracks_wider_control_group() {
        assert_eq!(traffic_lights_width(), 52.0);
        // Side cluster = max(actions=108, traffic_lights=52) = 108
        assert_eq!(titlebar_side_cluster_width(), 108.0);
    }

    #[test]
    fn snippet_panel_frame_clamps_to_viewport() {
        let frame = snippet_panel_frame(800.0, 620.0);
        assert_eq!(frame.top, 46.0);
        assert_eq!(frame.left, 12.0);
        assert_eq!(frame.width, 776.0);
        assert_eq!(frame.height, 558.0);
    }

    #[test]
    fn next_filtered_index_wraps_in_both_directions() {
        let visible = vec![0, 2, 4];
        assert_eq!(next_filtered_index(Some(4), &visible, 1), Some(0));
        assert_eq!(next_filtered_index(Some(0), &visible, -1), Some(4));
    }

    #[test]
    fn next_filtered_index_returns_none_for_empty_list() {
        assert_eq!(next_filtered_index(Some(0), &[], 1), None);
    }

    #[test]
    fn ai_settings_ime_target_maps_active_field() {
        assert_eq!(
            active_ai_settings_ime_target(Some(AiSettingsTextField::ApiKey)),
            Some(AiSettingsImeTarget::ApiKey)
        );
        assert_eq!(
            active_ai_settings_ime_target(Some(AiSettingsTextField::Model)),
            Some(AiSettingsImeTarget::Model)
        );
    }

    #[test]
    fn ai_settings_ime_replace_updates_model_text() {
        let mut config = AppConfig {
            ai_model: "gpt-4.1".to_string(),
            ..AppConfig::default()
        };

        let inserted = replace_text_in_ai_settings_ime_target(
            &mut config,
            AiSettingsImeTarget::Model,
            4..7,
            "4.1-mini",
        );

        assert_eq!(inserted, Some(4..12));
        assert_eq!(
            current_text_for_ai_settings_ime_target(&config, AiSettingsImeTarget::Model).as_deref(),
            Some("gpt-4.1-mini")
        );
    }

    #[test]
    fn direct_snippet_text_from_keystroke_prefers_key_char() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "process".into(),
            key_char: Some("日本".into()),
        };

        assert_eq!(
            direct_text_from_input_keystroke(&keystroke).as_deref(),
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
            direct_text_from_input_keystroke(&keystroke).as_deref(),
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
            direct_text_from_input_keystroke(&keystroke).as_deref(),
            Some("あ")
        );
    }

    #[test]
    fn should_defer_keystroke_to_input_method_skips_when_not_text() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "process".into(),
            key_char: None,
        };

        INPUT_METHOD_VK_PROCESSKEY.store(true, Ordering::Release);
        assert!(should_defer_keystroke_to_input_method(&keystroke));
    }

    #[test]
    fn should_defer_keystroke_to_input_method_allows_single_text_key() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "あ".into(),
            key_char: None,
        };

        INPUT_METHOD_VK_PROCESSKEY.store(true, Ordering::Release);
        assert!(!should_defer_keystroke_to_input_method(&keystroke));
    }

    #[test]
    fn should_defer_keystroke_to_input_method_defers_ascii_key_input() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "a".into(),
            key_char: Some("a".into()),
        };

        INPUT_METHOD_VK_PROCESSKEY.store(true, Ordering::Release);
        assert!(should_defer_keystroke_to_input_method(&keystroke));
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
}
