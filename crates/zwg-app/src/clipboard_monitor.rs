use std::path::Path;
use std::sync::{Mutex, Once, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use flume::Receiver;
use gpui::Window;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardMonitorEvent {
    pub sequence_number: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardCapture {
    pub sequence_number: u32,
    pub kind_label: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub note: Option<String>,
    pub tags: Vec<String>,
    pub source: String,
    pub created_label: String,
    pub captured_at_epoch_secs: u64,
}

#[derive(Debug)]
pub struct ClipboardListenerRegistration {
    #[cfg(target_os = "windows")]
    hwnd: windows::Win32::Foundation::HWND,
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct ClipboardMonitorState {
    hwnd_raw: Option<isize>,
    sender: Option<flume::Sender<ClipboardMonitorEvent>>,
}

#[cfg(target_os = "windows")]
fn clipboard_monitor_state() -> &'static Mutex<ClipboardMonitorState> {
    static STATE: OnceLock<Mutex<ClipboardMonitorState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ClipboardMonitorState::default()))
}

#[cfg(target_os = "windows")]
fn lock_clipboard_monitor_state() -> std::sync::MutexGuard<'static, ClipboardMonitorState> {
    clipboard_monitor_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn clipboard_getmessage_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, MSG, PM_REMOVE, WM_CLIPBOARDUPDATE,
    };

    if code >= 0 && wparam.0 == PM_REMOVE.0 as usize {
        let msg = unsafe { &*(lparam.0 as *const MSG) };
        if msg.message == WM_CLIPBOARDUPDATE {
            let (registered_hwnd_raw, sender) = {
                let state = lock_clipboard_monitor_state();
                (state.hwnd_raw, state.sender.clone())
            };

            if registered_hwnd_raw == Some(msg.hwnd.0 as isize) {
                if let Some(sender) = sender {
                    let _ = sender.send(ClipboardMonitorEvent {
                        sequence_number: unsafe { GetClipboardSequenceNumber() },
                    });
                }
            }
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(target_os = "windows")]
pub fn install_clipboard_monitor_hook() {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_GETMESSAGE};

    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let thread_id = GetCurrentThreadId();
        match SetWindowsHookExW(
            WH_GETMESSAGE,
            Some(clipboard_getmessage_hook_proc),
            None,
            thread_id,
        ) {
            Ok(_) => log::info!("Clipboard GetMessage hook installed"),
            Err(err) => log::error!("Failed to install clipboard hook: {}", err),
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn install_clipboard_monitor_hook() {}

#[cfg(target_os = "windows")]
pub fn register_clipboard_listener(
    window: &Window,
) -> Option<(
    ClipboardListenerRegistration,
    Receiver<ClipboardMonitorEvent>,
    u32,
)> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, GetClipboardSequenceNumber, RemoveClipboardFormatListener,
    };

    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return None;
    };
    let hwnd = HWND(raw.hwnd.get() as *mut core::ffi::c_void);

    {
        let previous_hwnd = lock_clipboard_monitor_state()
            .hwnd_raw
            .map(|raw| HWND(raw as *mut core::ffi::c_void));
        if let Some(previous_hwnd) = previous_hwnd.filter(|registered| *registered != hwnd) {
            let _ = unsafe { RemoveClipboardFormatListener(previous_hwnd) };
        }
    }

    if unsafe { AddClipboardFormatListener(hwnd) }.is_err() {
        return None;
    }

    let (sender, receiver) = flume::unbounded();
    {
        let mut state = lock_clipboard_monitor_state();
        state.hwnd_raw = Some(hwnd.0 as isize);
        state.sender = Some(sender);
    }

    Some((ClipboardListenerRegistration { hwnd }, receiver, unsafe {
        GetClipboardSequenceNumber()
    }))
}

#[cfg(not(target_os = "windows"))]
pub fn register_clipboard_listener(
    _window: &Window,
) -> Option<(
    ClipboardListenerRegistration,
    Receiver<ClipboardMonitorEvent>,
    u32,
)> {
    None
}

#[cfg(target_os = "windows")]
impl Drop for ClipboardListenerRegistration {
    fn drop(&mut self) {
        use windows::Win32::System::DataExchange::RemoveClipboardFormatListener;

        let _ = unsafe { RemoveClipboardFormatListener(self.hwnd) };

        let mut state = lock_clipboard_monitor_state();
        if state.hwnd_raw == Some(self.hwnd.0 as isize) {
            state.hwnd_raw = None;
            state.sender = None;
        }
    }
}

#[cfg(not(target_os = "windows"))]
impl Drop for ClipboardListenerRegistration {
    fn drop(&mut self) {}
}

#[cfg(target_os = "windows")]
pub fn snapshot_current_clipboard() -> Option<ClipboardCapture> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber,
        IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB, CF_DIBV5};
    use windows::Win32::System::SystemInformation::GetLocalTime;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowThreadProcessId};
    use windows::core::{PWSTR, w};

    fn with_open_clipboard<F, T>(f: F) -> Option<T>
    where
        F: FnOnce() -> Option<T>,
    {
        if unsafe { OpenClipboard(None) }.is_err() {
            return None;
        }

        let result = f();
        let _ = unsafe { CloseClipboard() };
        result
    }

    fn with_clipboard_data<F, T>(format: u32, f: F) -> Option<T>
    where
        F: FnOnce(*mut core::ffi::c_void, usize) -> Option<T>,
    {
        let global = HGLOBAL(unsafe { GetClipboardData(format).ok() }?.0);
        let size = unsafe { GlobalSize(global) };
        let data_ptr = unsafe { GlobalLock(global) };
        if data_ptr.is_null() {
            let _ = unsafe { GlobalUnlock(global) };
            return None;
        }
        let result = f(data_ptr, size);
        let _ = unsafe { GlobalUnlock(global) };
        result
    }

    fn read_unicode_text() -> Option<String> {
        use windows::Win32::System::Ole::CF_UNICODETEXT;

        with_clipboard_data(CF_UNICODETEXT.0 as u32, |data_ptr, size| {
            let wchar_len = size / 2;
            let wide = unsafe { std::slice::from_raw_parts(data_ptr.cast::<u16>(), wchar_len) };
            let terminator = wide
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(wide.len());
            String::from_utf16(&wide[..terminator]).ok()
        })
        .map(|text| text.replace("\r\n", "\n").replace('\r', "\n"))
        .filter(|text| !text.trim().is_empty())
    }

    fn clipboard_contains_image() -> bool {
        (unsafe { IsClipboardFormatAvailable(CF_BITMAP.0 as u32).is_ok() })
            || (unsafe { IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() })
            || (unsafe { IsClipboardFormatAvailable(CF_DIBV5.0 as u32).is_ok() })
    }

    fn clipboard_contains_html() -> bool {
        static HTML_FORMAT: OnceLock<u32> = OnceLock::new();
        let html_format =
            *HTML_FORMAT.get_or_init(|| unsafe { RegisterClipboardFormatW(w!("HTML Format")) });
        html_format != 0 && unsafe { IsClipboardFormatAvailable(html_format).is_ok() }
    }

    struct ProcessHandle(HANDLE);

    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    fn window_title(hwnd: HWND) -> Option<String> {
        let mut buffer = vec![0u16; 512];
        let copied = unsafe { GetWindowTextW(hwnd, buffer.as_mut_slice()) };
        if copied <= 0 {
            return None;
        }

        String::from_utf16(&buffer[..copied as usize]).ok()
    }

    fn process_name_for_window(hwnd: HWND) -> Option<String> {
        let mut process_id = 0_u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
        if process_id == 0 {
            return None;
        }

        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()? };
        let _handle = ProcessHandle(handle);
        let mut buffer = vec![0u16; 260];
        let mut size = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
            .ok()?;
        }

        let path = String::from_utf16(&buffer[..size as usize]).ok()?;
        let stem = Path::new(&path).file_stem()?.to_string_lossy();
        Some(friendly_source_name(&stem))
    }

    fn clipboard_source() -> String {
        let owner = unsafe { GetClipboardOwner().ok() };
        if let Some(owner) = owner.filter(|hwnd| !hwnd.is_invalid()) {
            if let Some(process_name) = process_name_for_window(owner) {
                return process_name;
            }
            if let Some(title) = window_title(owner).filter(|title| !title.trim().is_empty()) {
                return title;
            }
        }

        "Clipboard".to_string()
    }

    fn current_epoch_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn current_timestamp_label() -> String {
        let now = unsafe { GetLocalTime() };
        format!(
            "{:04}年{:02}月{:02}日 {:02}:{:02}:{:02}",
            now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
        )
    }

    let sequence_number = unsafe { GetClipboardSequenceNumber() };
    let source = clipboard_source();
    let captured_at_epoch_secs = current_epoch_secs();
    let created_label = current_timestamp_label();
    let contains_html = clipboard_contains_html();

    if let Some(text) = with_open_clipboard(read_unicode_text) {
        let kind_label = classify_text_kind(&text, contains_html);
        let (title, summary) = build_preview_fields(&text);
        let note = if contains_html {
            Some("HTML を含むクリップボード項目です。".to_string())
        } else {
            None
        };
        return Some(ClipboardCapture {
            sequence_number,
            kind_label: kind_label.to_string(),
            title,
            summary,
            content: text.clone(),
            note,
            tags: default_tags_for_kind(kind_label),
            source,
            created_label,
            captured_at_epoch_secs,
        });
    }

    if with_open_clipboard(|| Some(clipboard_contains_image())) == Some(true) {
        return Some(ClipboardCapture {
            sequence_number,
            kind_label: "IMAGE".to_string(),
            title: "画像クリップボード項目".to_string(),
            summary: "画像データがコピーされました。".to_string(),
            content: "[画像データ]".to_string(),
            note: Some("現在の詳細ビューでは画像プレビューは未対応です。".to_string()),
            tags: vec!["image".to_string()],
            source,
            created_label,
            captured_at_epoch_secs,
        });
    }

    None
}

#[cfg(not(target_os = "windows"))]
pub fn snapshot_current_clipboard() -> Option<ClipboardCapture> {
    None
}

fn build_preview_fields(text: &str) -> (String, String) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());

    let first_line = lines.next().unwrap_or_default();
    let remaining = lines.collect::<Vec<_>>().join(" ");
    let flattened = normalized.split_whitespace().collect::<Vec<_>>().join(" ");

    let title_source = if first_line.is_empty() {
        flattened.as_str()
    } else {
        first_line
    };
    let summary_source = if !remaining.is_empty() {
        remaining.as_str()
    } else if flattened.len() > title_source.len() {
        flattened
            .get(title_source.len()..)
            .map(str::trim)
            .unwrap_or_default()
    } else {
        ""
    };

    (
        truncate_with_ellipsis(title_source, 96),
        truncate_with_ellipsis(summary_source, 120),
    )
}

fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let char_count = trimmed.chars().count();
    if char_count <= max_chars {
        return trimmed.to_string();
    }

    let truncated = trimmed.chars().take(max_chars).collect::<String>();
    format!("{}…", truncated.trim_end())
}

fn classify_text_kind(text: &str, contains_html_format: bool) -> &'static str {
    let trimmed = text.trim();
    let lowered = trimmed.to_lowercase();

    if contains_html_format || looks_like_html(trimmed) {
        "HTML"
    } else if looks_like_email(trimmed) {
        "MAIL"
    } else if looks_like_url(trimmed) {
        "LINK"
    } else if looks_like_sql(&lowered) {
        "SQL"
    } else if looks_like_code(trimmed) {
        "CODE"
    } else if looks_like_date(trimmed) {
        "DATE"
    } else {
        "TEXT"
    }
}

fn default_tags_for_kind(kind_label: &str) -> Vec<String> {
    match kind_label {
        "HTML" => vec!["html".to_string()],
        "MAIL" => vec!["mail".to_string()],
        "LINK" => vec!["link".to_string()],
        "SQL" => vec!["sql".to_string()],
        "CODE" => vec!["code".to_string()],
        "DATE" => vec!["date".to_string()],
        "IMAGE" => vec!["image".to_string()],
        _ => Vec::new(),
    }
}

fn looks_like_html(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('<') && trimmed.contains('>') && trimmed.contains("</")
}

fn looks_like_email(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.contains('@')
        && !trimmed.contains(' ')
        && trimmed
            .split('@')
            .all(|part| !part.is_empty() && !part.contains('\n'))
}

fn looks_like_url(text: &str) -> bool {
    let trimmed = text.trim();
    (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && !trimmed.contains(char::is_whitespace)
}

fn looks_like_sql(text: &str) -> bool {
    let keywords = [
        "select ", "insert ", "update ", "delete ", "from ", "where ", "join ",
    ];
    keywords
        .iter()
        .filter(|keyword| text.contains(**keyword))
        .count()
        >= 2
}

fn looks_like_code(text: &str) -> bool {
    text.contains('{')
        || text.contains("=>")
        || text.contains("fn ")
        || text.contains("function ")
        || text.contains("let ")
        || text.contains("const ")
}

fn looks_like_date(text: &str) -> bool {
    text.contains('年')
        || text.contains('/')
        || text.contains('-') && text.chars().filter(|ch| ch.is_ascii_digit()).count() >= 6
}

fn friendly_source_name(process_stem: &str) -> String {
    match process_stem.to_ascii_lowercase().as_str() {
        "code" => "VS Code".to_string(),
        "chrome" => "Chrome".to_string(),
        "firefox" => "Firefox".to_string(),
        "msedge" => "Edge".to_string(),
        "datagrip" | "datagrip64" => "DataGrip".to_string(),
        "outlook" => "Outlook".to_string(),
        "pwsh" => "PowerShell".to_string(),
        "powershell" => "Windows PowerShell".to_string(),
        "explorer" => "Explorer".to_string(),
        "zwg" => "ZWG Terminal".to_string(),
        other => title_case_process_name(other),
    }
}

fn title_case_process_name(name: &str) -> String {
    name.split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        build_preview_fields, classify_text_kind, friendly_source_name, truncate_with_ellipsis,
    };

    #[test]
    fn build_preview_fields_uses_first_line_and_rest_excerpt() {
        let (title, summary) = build_preview_fields("hello world\nsecond line\nthird line");

        assert_eq!(title, "hello world");
        assert_eq!(summary, "second line third line");
    }

    #[test]
    fn classify_text_kind_distinguishes_supported_text_types() {
        assert_eq!(classify_text_kind("https://example.com", false), "LINK");
        assert_eq!(classify_text_kind("hello@example.com", false), "MAIL");
        assert_eq!(classify_text_kind("<div>Hello</div>", false), "HTML");
        assert_eq!(
            classify_text_kind("SELECT * FROM users WHERE id = 1", false),
            "SQL"
        );
        assert_eq!(classify_text_kind("function test() {}", false), "CODE");
    }

    #[test]
    fn friendly_source_name_maps_known_processes() {
        assert_eq!(friendly_source_name("code"), "VS Code");
        assert_eq!(friendly_source_name("msedge"), "Edge");
        assert_eq!(friendly_source_name("custom_tool"), "Custom Tool");
    }

    #[test]
    fn truncate_with_ellipsis_caps_long_preview_text() {
        let truncated = truncate_with_ellipsis("abcdefghijklmnopqrstuvwxyz", 8);

        assert_eq!(truncated, "abcdefgh…");
    }
}
