//! Terminal pane — GPUI view that renders the terminal and handles input

use std::collections::VecDeque;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex as StdMutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use gpui::ElementInputHandler;
use gpui::*;

use super::TerminalSettings;
#[cfg(feature = "ghostty_vt")]
use super::gpu_view::{
    CursorOverlay, GpuTerminalState, gpu_terminal_canvas, snapshot_can_present_natively,
};
#[cfg(feature = "ghostty_vt")]
use super::grid_renderer::resolve_cursor_cell;
use super::grid_renderer::{
    GlyphCache, GridRendererConfig, SelectionPoint, TerminalSnapshot, col_to_char_index,
    damage_spans_from_terminal_row, full_row_damage, glyph_instances_in_damage,
    grid_cells_from_parts, patch_cells_in_damage, patch_glyph_instances_in_damage, terminal_canvas,
};
use super::pty::{ConPtyConfig, spawn_pty};
use super::surface::TerminalSurface;
use super::win32_input::encode_win32_input_text;
#[cfg(feature = "ghostty_vt")]
use parking_lot::Mutex;

const HORIZONTAL_TEXT_PADDING: f32 = 4.0;
// --- Adaptive frame pacing ---------------------------------------------------
// Two modes: NORMAL (interactive / PSReadLine) and FAST (sustained output like
// Claude Code /fast).  The event loop switches automatically based on how many
// consecutive frames contained changes.
const FRAME_COALESCE_NORMAL_MICROS: u64 = 1_667; // ~600 Hz
const FRAME_COALESCE_FAST_MICROS: u64 = 1_667; // ~600 Hz (match normal for sustained throughput)
const SETTLE_NORMAL_MILLIS: u64 = 1;
const SETTLE_FAST_MILLIS: u64 = 0;
const RETRY_LIMIT_NORMAL: usize = 4;
const RETRY_LIMIT_FAST: usize = 2;
const SWEEPS_NORMAL: usize = 6;
const SWEEPS_FAST: usize = 10;
/// Consecutive changed-frames before entering fast pacing mode.
const FAST_PACING_ENTER: u32 = 2;
/// Consecutive idle frames before reverting to normal pacing mode.
const FAST_PACING_EXIT: u32 = 4;
const CROSS_ROUTE_DUPLICATE_WINDOW_MS: u64 = 250;
const SAME_ROUTE_COMMIT_DUPLICATE_WINDOW_MS: u64 = 100;

/// Interval between periodic session saves to RAMdisk (30 seconds).
const SESSION_SAVE_INTERVAL: Duration = Duration::from_secs(30);

/// Session state file name on RAMdisk.
const SESSION_SNAPSHOT_FILE: &str = "session_snapshot.json";

/// Session state snapshot persisted to RAMdisk for fast save/restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub scroll_offset: i64,
    pub timestamp: u64,
}

impl SessionSnapshot {
    /// Save snapshot to RAMdisk tmp/ directory.
    /// No-op if ZWG_RAMDISK is not set.
    fn save_to_ramdisk(&self) {
        let Some(ramdisk) = std::env::var("ZWG_RAMDISK").ok() else {
            return;
        };
        let path = PathBuf::from(&ramdisk)
            .join("tmp")
            .join(SESSION_SNAPSHOT_FILE);
        match serde_json::to_string(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::warn!("Failed to save session snapshot to {}: {e}", path.display());
                } else {
                    log::debug!("Session snapshot saved to {}", path.display());
                }
            }
            Err(e) => log::warn!("Failed to serialize session snapshot: {e}"),
        }
    }

    /// Load snapshot from RAMdisk tmp/ directory.
    /// Returns None if ZWG_RAMDISK is not set or file doesn't exist.
    pub fn load_from_ramdisk() -> Option<Self> {
        let ramdisk = std::env::var("ZWG_RAMDISK").ok()?;
        let path = PathBuf::from(&ramdisk)
            .join("tmp")
            .join(SESSION_SNAPSHOT_FILE);
        let content = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<Self>(&content) {
            Ok(snap) => {
                log::info!(
                    "Restored session snapshot from RAMdisk: cwd={}, {}x{}, scroll={}",
                    snap.cwd, snap.cols, snap.rows, snap.scroll_offset
                );
                Some(snap)
            }
            Err(e) => {
                log::warn!("Invalid session snapshot at {}: {e}", path.display());
                None
            }
        }
    }
}
/// Fallback values — replaced at runtime by measured font metrics
const CELL_WIDTH_FALLBACK: f32 = 8.4;
const CELL_HEIGHT_FALLBACK: f32 = 19.5;
pub const CELL_WIDTH_ESTIMATE: f32 = CELL_WIDTH_FALLBACK;
pub const CELL_HEIGHT_ESTIMATE: f32 = CELL_HEIGHT_FALLBACK;
pub const WINDOW_CHROME_HEIGHT: f32 = 60.0;

#[cfg(feature = "ghostty_vt")]
#[derive(Clone)]
struct GhosttyRowUpdate {
    row: u16,
    text: String,
    style_runs: Vec<ghostty_vt::StyleRun>,
}

#[cfg(not(feature = "ghostty_vt"))]
#[derive(Clone)]
struct FallbackRowUpdate {
    row: u16,
    text: String,
    generation: u64,
}

#[cfg(feature = "ghostty_vt")]
fn apply_ghostty_row_update(
    cached_row: &mut super::grid_renderer::CachedTerminalRow,
    row_update: GhosttyRowUpdate,
    term_cols: u16,
    default_fg: u32,
    default_bg: u32,
    force_full: bool,
) -> bool {
    let next_cells = grid_cells_from_parts(
        &row_update.text,
        &row_update.style_runs,
        term_cols,
        default_fg,
        default_bg,
    );
    let damage_spans = if force_full {
        full_row_damage(term_cols)
    } else {
        damage_spans_from_terminal_row(
            &cached_row.cells,
            &cached_row.style_runs,
            &next_cells,
            &row_update.style_runs,
            term_cols,
            default_fg,
            default_bg,
        )
    };
    let cells = if force_full {
        next_cells.clone()
    } else {
        patch_cells_in_damage(&cached_row.cells, &next_cells, &damage_spans)
    };
    let glyph_instances = if force_full {
        super::grid_renderer::glyph_instances_from_cells(&cells, row_update.row)
    } else {
        patch_glyph_instances_in_damage(
            &cached_row.glyph_instances,
            &next_cells,
            row_update.row,
            &damage_spans,
        )
    };
    let damaged_glyph_instances = glyph_instances_in_damage(&cells, row_update.row, &damage_spans);
    let row_changed = !damage_spans.is_empty()
        || cached_row.text.as_ref() != row_update.text.as_str()
        || cached_row.style_runs != row_update.style_runs;

    cached_row.text = SharedString::from(row_update.text);
    cached_row.style_runs = row_update.style_runs;
    cached_row.cells = cells;
    cached_row.glyph_instances = glyph_instances;
    cached_row.damage_spans = damage_spans;
    cached_row.damaged_glyph_instances = damaged_glyph_instances;
    row_changed
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UserInputSource {
    KeyDown,
    TextCommit,
    ImeEndComposition,
}

impl UserInputSource {
    fn is_commit_source(self) -> bool {
        matches!(self, Self::TextCommit | Self::ImeEndComposition)
    }
}

/// Measure the actual monospace cell dimensions from the configured font.
/// Cell width = max advance width of representative monospace glyphs.
/// Cell height = ascent + descent (no extra leading — required for
/// box-drawing characters │─┌┐└┘ to connect between adjacent rows).
fn measure_cell_dimensions(cx: &App, font_family: &str, font_size_px: f32) -> (f32, f32) {
    let text_system = cx.text_system();
    let font_desc = font(SharedString::from(font_family.to_string()));
    let font_id = text_system.resolve_font(&font_desc);
    let font_size = px(font_size_px);

    let cell_width = ['M', 'W', '@', '0', '█', '│']
        .into_iter()
        .filter_map(|ch| text_system.advance(font_id, font_size, ch).ok())
        .map(|size| {
            let w: f32 = size.width.into();
            if w > 1.0 { w } else { CELL_WIDTH_FALLBACK }
        })
        .fold(CELL_WIDTH_FALLBACK, f32::max);

    let ascent: f32 = text_system.ascent(font_id, font_size).into();
    let descent: f32 = text_system.descent(font_id, font_size).into();
    // descent may be negative (OpenType convention) — use abs
    let cell_height = ascent + descent.abs();
    let cell_height = if cell_height > font_size_px {
        cell_height
    } else {
        CELL_HEIGHT_FALLBACK
    };

    // Use exact measured values — NO rounding.
    // cell_height = ascent + descent means paint_line's padding_top = 0,
    // so glyphs fill the full cell height with no gaps between rows.
    // Backgrounds are painted manually via paint_quad at grid positions.
    (cell_width, cell_height)
}

// Figma-aligned chrome colors for status text
const SUBTEXT0: u32 = 0x8E8E93;
const SURFACE0: u32 = 0x48484A;
const RED: u32 = 0xFF5F57;
// ── IME hook: fix Japanese/Chinese/Korean input for gpui 0.2.2 ──────
//
// gpui 0.2.2 calls TranslateMessage inside WndProc with a synthetic MSG
// (time=0), preventing IME from generating WM_IME_COMPOSITION.
// WH_GETMESSAGE hook intercepts VK_PROCESSKEY and calls TranslateMessage
// with the real MSG so IME composition works correctly.

static IME_VK_PROCESSKEY: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static IME_COMPOSITION_RESULT_QUEUE: StdMutex<VecDeque<String>> = StdMutex::new(VecDeque::new());

fn terminal_ime_trace_enabled() -> bool {
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

#[cfg(target_os = "windows")]
fn queue_ime_endcomposition_text(hwnd: windows::Win32::Foundation::HWND) {
    let Some(text) = read_ime_result_text(hwnd) else {
        if terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM queue endcomposition skipped (no text read) wnd=0x{:X}",
                hwnd.0 as usize
            );
        }
        return;
    };
    if terminal_ime_trace_enabled() {
        log::debug!("IME_TERM queued endcomposition text={:?}", text);
    }

    match IME_COMPOSITION_RESULT_QUEUE.lock() {
        Ok(mut queue) => queue.push_back(text),
        Err(err) => err.into_inner().push_back(text),
    }
}

#[cfg(target_os = "windows")]
fn take_ime_endcomposition_texts() -> Vec<String> {
    match IME_COMPOSITION_RESULT_QUEUE.lock() {
        Ok(mut queue) => queue.drain(..).collect(),
        Err(err) => err.into_inner().drain(..).collect(),
    }
}

#[cfg(target_os = "windows")]
fn take_ime_endcomposition_texts_for_terminal(input_suppressed: bool) -> Vec<String> {
    let texts = take_ime_endcomposition_texts();
    if input_suppressed {
        if !texts.is_empty() && terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM dropped {} queued endcomposition item(s) while input was suppressed",
                texts.len()
            );
        }
        Vec::new()
    } else {
        texts
    }
}

#[cfg(target_os = "windows")]
fn read_ime_result_text(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::Win32::UI::Input::Ime::{
        GCS_COMPREADSTR, GCS_COMPSTR, GCS_RESULTREADSTR, GCS_RESULTSTR, IME_COMPOSITION_STRING,
        ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
    };

    let himc = unsafe { ImmGetContext(hwnd) };

    if himc.0.is_null() {
        if terminal_ime_trace_enabled() {
            log::debug!("IME_TERM no IME context for wnd=0x{:X}", hwnd.0 as usize);
        }
        return None;
    }

    fn read_string_for_kind(
        himc: windows::Win32::UI::Input::Ime::HIMC,
        kind: IME_COMPOSITION_STRING,
        kind_name: &str,
    ) -> Option<String> {
        use std::ffi::c_void;

        let required_bytes_raw = unsafe { ImmGetCompositionStringW(himc, kind, None, 0) };
        if required_bytes_raw <= 0 {
            if terminal_ime_trace_enabled() {
                log::debug!("IME_TERM {} size={}", kind_name, required_bytes_raw);
            }
            return None;
        }
        let required_bytes = required_bytes_raw as usize;

        let mut buffer = vec![0u8; required_bytes];
        let buffer_len = u32::try_from(buffer.len()).ok()?;
        let written = unsafe {
            ImmGetCompositionStringW(
                himc,
                kind,
                Some(buffer.as_mut_ptr().cast::<c_void>()),
                buffer_len,
            )
        };
        if written <= 0 {
            if terminal_ime_trace_enabled() {
                log::debug!("IME_TERM {} written={}", kind_name, written);
            }
            return None;
        }
        let written_len = (written as usize).min(buffer.len());

        let bytes = Vec::from(&buffer[..written_len]);
        if bytes.is_empty() {
            if terminal_ime_trace_enabled() {
                log::debug!("IME_TERM {} bytes empty", kind_name);
            }
            return None;
        }

        if bytes.len() % 2 != 0 {
            if terminal_ime_trace_enabled() {
                log::debug!(
                    "IME_TERM {} odd byte length for utf16-le: {}",
                    kind_name,
                    bytes.len()
                );
            }
        }

        let mut u16_units = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        while u16_units.last().is_some_and(|value| *value == 0) {
            u16_units.pop();
        }
        if u16_units.is_empty() {
            if terminal_ime_trace_enabled() {
                log::debug!("IME_TERM {} utf16 units empty", kind_name);
            }
            return None;
        }
        if let Ok(text) = String::from_utf16(&u16_units) {
            if terminal_ime_trace_enabled() {
                log::debug!(
                    "IME_TERM {} decoded utf16-le units={} bytes={} -> {:?}",
                    kind_name,
                    u16_units.len(),
                    bytes.len(),
                    text
                );
            }
            return Some(text);
        }

        let text = String::from_utf16_lossy(&u16_units);
        if terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM {} utf16-le decode lossy units={} bytes={} -> {:?}",
                kind_name,
                u16_units.len(),
                bytes.len(),
                text
            );
        }
        if text.is_empty() {
            if terminal_ime_trace_enabled() {
                log::debug!("IME_TERM {} text empty", kind_name);
            }
            None
        } else {
            Some(text)
        }
    }

    let result = read_string_for_kind(himc, GCS_RESULTSTR, "RESULTSTR")
        .or_else(|| read_string_for_kind(himc, GCS_RESULTREADSTR, "RESULTREADSTR"))
        .or_else(|| read_string_for_kind(himc, GCS_COMPSTR, "COMPSTR"))
        .or_else(|| read_string_for_kind(himc, GCS_COMPREADSTR, "COMPREADSTR"));

    unsafe {
        let _ = ImmReleaseContext(hwnd, himc);
    };

    if terminal_ime_trace_enabled() {
        log::debug!("IME_TERM IME read result -> {:?}", result);
    }
    result
}

fn log_terminal_ime_keystroke(context: &str, keystroke: &Keystroke, detail: &str) {
    if !terminal_ime_trace_enabled() {
        return;
    }

    log::debug!(
        "IME_TERM [{}] key={} key_char={:?} ctrl:{} alt:{} shift:{} detail={}",
        context,
        keystroke.key,
        keystroke.key_char,
        keystroke.modifiers.control,
        keystroke.modifiers.alt,
        keystroke.modifiers.shift,
        detail
    );
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn ime_getmessage_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, MSG, PM_REMOVE, TranslateMessage, WM_IME_COMPOSITION,
        WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN,
    };

    if code >= 0 && wparam.0 == PM_REMOVE.0 as usize {
        unsafe {
            let msg = &*(lparam.0 as *const MSG);
            match msg.message {
                message if message == WM_IME_STARTCOMPOSITION => {
                    if terminal_ime_trace_enabled() {
                        log::debug!(
                            "IME_TERM_CMP_START time={} wparam=0x{:X} lparam=0x{:X}",
                            msg.time,
                            msg.wParam.0,
                            msg.lParam.0
                        );
                    }
                }
                message if message == WM_IME_COMPOSITION => {
                    // NOTE: Do NOT queue text here.  gpui 0.2.2 handles
                    // WM_IME_COMPOSITION → GCS_RESULTSTR → replace_text_in_range
                    // directly, so queueing the same text from the hook causes
                    // double delivery.  WM_IME_ENDCOMPOSITION (below) still
                    // queues as a safety net in case gpui fails to deliver.
                    if terminal_ime_trace_enabled() {
                        log::debug!(
                            "IME_TERM_CMP message time={} wparam=0x{:X} lparam=0x{:X}",
                            msg.time,
                            msg.wParam.0,
                            msg.lParam.0
                        );
                    }
                }
                message if message == WM_IME_ENDCOMPOSITION => {
                    queue_ime_endcomposition_text(msg.hwnd);
                    if terminal_ime_trace_enabled() {
                        log::debug!(
                            "IME_TERM_CMP_END time={} wparam=0x{:X} lparam=0x{:X}",
                            msg.time,
                            msg.wParam.0,
                            msg.lParam.0
                        );
                    }
                }
                _ => {}
            }
            if msg.message == WM_KEYDOWN {
                let vk = (msg.wParam.0 & 0xFFFF) as u16;
                if terminal_ime_trace_enabled() {
                    log::debug!(
                        "IME_TERM_HOOK raw keydown vk=0x{:04X} code={} wparam=0x{:X} lparam=0x{:X}",
                        vk,
                        msg.message,
                        msg.wParam.0,
                        msg.lParam.0,
                    );
                }
                if vk == 0xE5 {
                    if terminal_ime_trace_enabled() {
                        log::debug!("IME_TERM_HOOK VK_PROCESSKEY detected -> latch");
                    }
                    // VK_PROCESSKEY: IME is processing this key.
                    // Call TranslateMessage with the ORIGINAL MSG (real time/pt).
                    let _ = TranslateMessage(msg as *const MSG);
                    IME_VK_PROCESSKEY.store(true, Ordering::Release);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(target_os = "windows")]
fn install_ime_hook() {
    use std::sync::Once;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_GETMESSAGE};

    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let thread_id = GetCurrentThreadId();
        match SetWindowsHookExW(
            WH_GETMESSAGE,
            Some(ime_getmessage_hook_proc),
            None,
            thread_id,
        ) {
            Ok(_) => log::info!("IME GetMessage hook installed"),
            Err(e) => log::error!("Failed to install IME hook: {}", e),
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn install_ime_hook() {}

/// Read clipboard text using native Windows API as fallback when GPUI fails.
#[cfg(target_os = "windows")]
fn read_clipboard_native_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }
        let result = (|| {
            let global = HGLOBAL(GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?.0);
            let size = GlobalSize(global);
            if size == 0 {
                return None;
            }
            let ptr = GlobalLock(global);
            if ptr.is_null() {
                return None;
            }
            let u16_count = size / 2;
            let slice = std::slice::from_raw_parts(ptr as *const u16, u16_count);
            // Find null terminator
            let len = slice.iter().position(|&c| c == 0).unwrap_or(u16_count);
            let text = String::from_utf16_lossy(&slice[..len]);
            let _ = GlobalUnlock(global);
            if text.is_empty() { None } else { Some(text) }
        })();
        let _ = CloseClipboard();
        result
    }
}

#[cfg(not(target_os = "windows"))]
fn read_clipboard_native_text() -> Option<String> {
    None
}

fn normalize_terminal_newlines(text: &str) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    normalized.into_bytes()
}

fn quote_path_for_terminal(path: &str) -> String {
    if path.contains([' ', '\t']) {
        format!("\"{}\"", path.replace('"', "\\\""))
    } else {
        path.to_string()
    }
}

fn format_dropped_paths(paths: &ExternalPaths) -> String {
    paths
        .paths()
        .iter()
        .map(|path| quote_path_for_terminal(&path.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn viewport_rows_to_refresh(
    rows: u16,
    force_full: bool,
    scroll_delta: i32,
    dirty_rows: Option<Vec<u16>>,
) -> Vec<u16> {
    if rows == 0 {
        return Vec::new();
    }

    if force_full || scroll_delta != 0 {
        return (0..rows).collect();
    }

    dirty_rows.unwrap_or_else(|| (0..rows).collect())
}

fn scroll_lines_from_wheel_delta(
    delta: ScrollDelta,
    cell_height: f32,
    line_remainder: &mut f32,
) -> i32 {
    let line_height = cell_height.max(1.0);
    let line_delta = match delta {
        ScrollDelta::Lines(delta) => delta.y,
        ScrollDelta::Pixels(delta) => {
            let pixels: f32 = delta.y.into();
            pixels / line_height
        }
    };

    // GPUI reports positive Y for wheel-up on Windows. Viewport scrolling uses
    // positive lines to move upward into scrollback, so invert once here.
    let total = *line_remainder - line_delta;
    let whole_lines = if total >= 0.0 {
        total.floor() as i32
    } else {
        total.ceil() as i32
    };
    *line_remainder = total - whole_lines as f32;
    whole_lines
}

fn terminal_layout_size(
    viewport_size: Size<Pixels>,
    last_bounds: Option<Bounds<Pixels>>,
) -> (f32, f32) {
    if let Some(bounds) = last_bounds {
        let width: f32 = bounds.size.width.into();
        let height: f32 = bounds.size.height.into();
        return (width.max(1.0), height.max(100.0));
    }

    let width: f32 = viewport_size.width.into();
    let height: f32 = viewport_size.height.into();
    (width.max(1.0), (height - WINDOW_CHROME_HEIGHT).max(100.0))
}

#[cfg(target_os = "windows")]
fn terminal_input_method_native_mode_active() -> bool {
    use windows::Win32::UI::Input::Ime::{
        IME_CMODE_FULLSHAPE, IME_CMODE_NATIVE, IME_CONVERSION_MODE, IME_SENTENCE_MODE,
        ImmGetContext, ImmGetConversionStatus, ImmGetOpenStatus, ImmReleaseContext,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    use windows::Win32::System::Threading::GetCurrentProcessId;

    unsafe {
        let hwnd = GetForegroundWindow();
        // Validate foreground window belongs to our process to avoid
        // reading another application's IME context after Alt+Tab.
        let mut window_pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
        if window_pid != GetCurrentProcessId() {
            return false;
        }
        let himc = ImmGetContext(hwnd);
        if himc.0.is_null() {
            return false;
        }

        let open = ImmGetOpenStatus(himc).as_bool();
        let mut conversion = IME_CONVERSION_MODE(0);
        let mut sentence = IME_SENTENCE_MODE(0);
        let has_conversion_status = ImmGetConversionStatus(
            himc,
            Some(&mut conversion as *mut IME_CONVERSION_MODE),
            Some(&mut sentence as *mut IME_SENTENCE_MODE),
        )
        .as_bool();
        let _ = ImmReleaseContext(hwnd, himc);

        if !open {
            return false;
        }

        if !has_conversion_status {
            return true;
        }

        (conversion.0 & IME_CMODE_NATIVE.0) != 0 || (conversion.0 & IME_CMODE_FULLSHAPE.0) != 0
    }
}

#[cfg(not(target_os = "windows"))]
fn terminal_input_method_native_mode_active() -> bool {
    false
}

fn should_defer_keystroke_to_ime(ks: &Keystroke, ime_processkey_pending: bool) -> bool {
    should_defer_keystroke_to_ime_with_state(
        ks,
        ime_processkey_pending,
        terminal_input_method_native_mode_active(),
    )
}

fn should_defer_keystroke_to_ime_with_state(
    ks: &Keystroke,
    ime_processkey_pending: bool,
    ime_native_mode_active: bool,
) -> bool {
    if terminal_ime_trace_enabled() {
        log::debug!(
            "IME_TERM should_defer_keystroke_to_ime key={} pending={} ime_active={} key_char={:?}",
            ks.key,
            ime_processkey_pending,
            ime_native_mode_active,
            ks.key_char
        );
    }

    if !ime_processkey_pending {
        return false;
    }

    if !ime_native_mode_active {
        return false;
    }

    // 制御キーはIME処理と無関係 → deferせずターミナル直通
    if matches!(
        ks.key.as_ref(),
        "escape"
            | "tab"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "delete"
            | "insert"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
    ) {
        return false;
    }

    // key_char が存在する場合、文字種で判定
    if let Some(ref key_char) = ks.key_char {
        if !key_char.is_empty() {
            // ASCII文字はIME処理中なので defer（ローマ字入力中の a, k, i 等）
            // 非ASCII文字はIME確定文字なので defer 解除（あ, 漢字 等）
            let defer = key_char.chars().all(|ch| ch.is_ascii());
            if terminal_ime_trace_enabled() {
                log::debug!(
                    "IME_TERM should_defer key_char={:?} all_ascii={} -> defer={}",
                    key_char,
                    defer,
                    defer
                );
            }
            return defer;
        }
    }

    // key_char が空または None → defer（IMEがまだ処理中）
    true
}

#[cfg(target_os = "windows")]
fn should_route_keystroke_via_text_input(ks: &Keystroke) -> bool {
    should_route_keystroke_via_text_input_with_state(
        ks,
        IME_VK_PROCESSKEY.load(Ordering::Acquire),
        terminal_input_method_native_mode_active(),
    )
}

#[cfg(target_os = "windows")]
fn should_route_keystroke_via_text_input_with_state(
    ks: &Keystroke,
    ime_processkey_pending: bool,
    ime_native_mode_active: bool,
) -> bool {
    if ks.modifiers.control || ks.modifiers.alt {
        return false;
    }

    if !ime_processkey_pending {
        return false;
    }

    if !ime_native_mode_active {
        return false;
    }

    ks.key_char
        .as_ref()
        .is_some_and(|key_char| !key_char.is_empty() && key_char.chars().any(|ch| !ch.is_ascii()))
}

#[cfg(not(target_os = "windows"))]
fn should_route_keystroke_via_text_input(_ks: &Keystroke) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn should_forward_replace_text_to_terminal(text: &str, _ime_composing: bool) -> bool {
    // On Windows, all non-empty text from replace_text_in_range should be forwarded.
    // Both IME composition commits and regular keystrokes flow through this path.
    !text.is_empty()
}

#[cfg(not(target_os = "windows"))]
fn should_forward_replace_text_to_terminal(text: &str, _ime_composing: bool) -> bool {
    !text.is_empty()
}

#[cfg(target_os = "windows")]
fn text_to_terminal_bytes(text: &str, win32_input_mode: bool) -> Vec<u8> {
    if win32_input_mode {
        encode_win32_input_text(text)
    } else {
        text.as_bytes().to_vec()
    }
}

#[cfg(not(target_os = "windows"))]
fn text_to_terminal_bytes(text: &str, _win32_input_mode: bool) -> Vec<u8> {
    text.as_bytes().to_vec()
}

/// Terminal connection state — two-phase init pattern
#[derive(Debug)]
enum TerminalState {
    /// PTY is being spawned in background
    Pending,
    /// PTY is connected and running
    Running,
    /// PTY spawn failed
    Failed(String),
}

/// Terminal pane: GPUI component that wraps a TerminalSurface
pub struct TerminalPane {
    surface: TerminalSurface,
    focus_handle: FocusHandle,
    input_suppressed: Arc<AtomicBool>,
    state: TerminalState,
    snapshot: TerminalSnapshot,
    /// Cached cell dimensions
    cell_width: f32,
    cell_height: f32,
    font_family: SharedString,
    font_size: f32,
    cursor_blink: bool,
    copy_on_select: bool,
    gpu_acceleration: bool,
    fg_color: u32,
    bg_color: u32,
    background_image_path: Option<String>,
    background_image_opacity: f32,
    global_hotkeys: Vec<String>,
    blink_started_at: Instant,
    /// Current terminal size in cells
    term_cols: u16,
    term_rows: u16,
    /// Last known layout size for resize detection
    last_width: f32,
    last_height: f32,
    last_bounds: Option<Bounds<Pixels>>,
    selection_anchor: Option<SelectionPoint>,
    selection_head: Option<SelectionPoint>,
    is_selecting: bool,
    ime_composing: bool,
    wheel_scroll_line_remainder: f32,
    recent_user_inputs: VecDeque<(UserInputSource, Vec<u8>, Instant)>,
    /// Keystrokes buffered while PTY is still connecting (Pending state)
    pending_input: Vec<u8>,
    pending_process_exit_status: Option<i32>,
    /// When true, the pane is automatically closed when the process exits.
    /// Set for teammate panes spawned via IPC split-window with a command.
    auto_close: bool,
    /// Numeric pane ID for auto-close (set together with auto_close)
    auto_close_pane_id: Option<u32>,
    /// Set by the blink timer to indicate only cursor visibility changed — no content refresh needed.
    cursor_blink_pending: bool,
    /// Cross-frame glyph layout cache — avoids reshaping unchanged glyphs every paint
    glyph_cache: GlyphCache,
    /// DX12 GPU renderer state — bypasses GPUI text shaping when available
    #[cfg(feature = "ghostty_vt")]
    gpu_state: Option<Arc<Mutex<GpuTerminalState>>>,
    #[cfg(not(feature = "ghostty_vt"))]
    row_generations: Vec<u64>,
    /// Subscription for focus-in listener (lazy-initialized in render)
    _focus_in_sub: Option<gpui::Subscription>,
    /// Right-click context menu state
    context_menu_visible: bool,
    context_menu_position: Point<Pixels>,
    /// Scrollbar: current viewport scroll offset (lines above bottom, 0 = at bottom)
    scroll_offset: i64,
    /// Scrollbar: whether user is dragging the scrollbar thumb
    scrollbar_dragging: bool,
    /// Scrollbar: max scrollback capacity (from settings) for scroll clamping
    max_scrollback_lines: usize,
    /// Working directory this pane was started with (for session snapshot)
    initial_working_directory: Option<String>,
    /// Last time session was saved to RAMdisk (for periodic 30s saves)
    last_session_save: Instant,
}

impl TerminalPane {
    pub fn new(shell: &str, settings: TerminalSettings, cx: &mut Context<Self>) -> Self {
        Self::new_inner(shell, None, None, Vec::new(), settings, cx)
    }

    /// Create a new terminal pane with a specific command, environment variables,
    /// and pre-assigned pane_id. Used by IPC split-window to spawn teammate agents.
    pub fn new_with_command(
        shell: &str,
        command: Option<String>,
        working_directory: Option<String>,
        extra_env: Vec<(String, String)>,
        pane_id: u32,
        settings: TerminalSettings,
        cx: &mut Context<Self>,
    ) -> Self {
        // If a command is provided, wrap it in a non-interactive shell so the PTY
        // process exits when the command finishes (critical for auto_close).
        let effective_shell = match command {
            Some(cmd) => {
                #[cfg(windows)]
                { format!("cmd.exe /C {}", cmd) }
                #[cfg(not(windows))]
                { format!("sh -c '{}'", cmd.replace('\'', "'\\''")) }
            }
            None => shell.to_string(),
        };
        Self::new_inner(&effective_shell, working_directory, Some(pane_id), extra_env, settings, cx)
    }

    fn new_inner(
        shell: &str,
        working_directory: Option<String>,
        assigned_pane_id: Option<u32>,
        extra_env: Vec<(String, String)>,
        settings: TerminalSettings,
        cx: &mut Context<Self>,
    ) -> Self {
        // Install IME hook once per process
        install_ime_hook();

        let focus_handle = cx.focus_handle();
        let mut surface =
            TerminalSurface::new(settings.cols, settings.rows, settings.scrollback_lines);
        surface.set_default_colors(settings.fg_color, settings.bg_color);
        let event_rx = surface.take_event_rx();

        // Try to restore session from RAMdisk (CWD override)
        let restored_session = SessionSnapshot::load_from_ramdisk();
        let effective_working_directory = match (&working_directory, &restored_session) {
            (Some(wd), _) => Some(wd.clone()),
            (None, Some(snap)) if !snap.cwd.is_empty() => Some(snap.cwd.clone()),
            _ => None,
        };
        let saved_working_directory = effective_working_directory.clone();

        // Phase A: Return immediately with Pending state (<1ms)
        // Phase B: Spawn PTY in background thread
        let shell_owned = shell.to_string();
        let initial_cols = settings.cols;
        let initial_rows = settings.rows;
        // Use pre-assigned pane_id if provided, otherwise allocate a new one.
        // This prevents the double-allocation bug where split() and new() each
        // called next_pane_id(), causing SplitNode pane_id and TMUX_PANE to diverge.
        let pane_id = assigned_pane_id.unwrap_or_else(|| crate::split::next_pane_id());
        let mut env = crate::terminal::pty::zwg_env_vars(pane_id);
        env.extend(extra_env);
        cx.spawn(
            async move |this: WeakEntity<TerminalPane>, cx: &mut AsyncApp| {
                // Run ConPTY creation on background executor (off UI thread)
                let shell_for_spawn = shell_owned.clone();
                let pty_result = cx
                    .background_executor()
                    .spawn(async move {
                        let config = ConPtyConfig {
                            shell: shell_for_spawn,
                            working_directory: effective_working_directory,
                            cols: initial_cols,
                            rows: initial_rows,
                            env,
                        };
                        spawn_pty(config)
                    })
                    .await;

                // Phase C: Attach PTY to surface on executor context
                let _ = this.update(cx, |pane: &mut TerminalPane, cx| {
                    match pty_result {
                        Ok(pty) => {
                            if let Err(e) = pane.surface.attach_pty(Arc::new(pty)) {
                                pane.state = TerminalState::Failed(e.to_string());
                                log::error!("Failed to attach PTY: {}", e);
                            } else {
                                pane.state = TerminalState::Running;
                                pane.refresh_snapshot(true);
                                log::info!("PTY connected for shell: {}", shell_owned);

                                // Flush any keystrokes buffered during Pending state
                                if !pane.pending_input.is_empty() {
                                    let buf = std::mem::take(&mut pane.pending_input);
                                    let _ = pane.surface.write_input(&buf);
                                }
                            }
                        }
                        Err(e) => {
                            pane.state = TerminalState::Failed(e.to_string());
                            log::error!("Failed to spawn shell: {}", e);
                        }
                    }
                    cx.notify();
                });
            },
        )
        .detach();

        // Wait for PTY output, then coalesce updates with an upper bound matching
        // a 600Hz frame budget. This keeps bursty PTY output from flooding the UI
        // while still allowing high refresh-rate panels to update promptly.
        cx.spawn(
            async move |this: WeakEntity<TerminalPane>, cx: &mut AsyncApp| {
                let mut last_presented: Option<std::time::Instant> = None;
                // Adaptive pacing state
                let mut consecutive_busy: u32 = 0;
                let mut consecutive_idle: u32 = 0;

                loop {
                    let Ok(event) = event_rx.recv_async().await else {
                        break;
                    };

                    let mut process_exit_status = match event {
                        super::surface::TerminalEvent::ProcessExited(code) => Some(code),
                        _ => None,
                    };
                    while let Ok(event) = event_rx.try_recv() {
                        if let super::surface::TerminalEvent::ProcessExited(code) = event {
                            process_exit_status = Some(code);
                        }
                    }

                    // --- Select pacing parameters based on current mode ------
                    let fast_mode = consecutive_busy >= FAST_PACING_ENTER;
                    let frame_budget = Duration::from_micros(if fast_mode {
                        FRAME_COALESCE_FAST_MICROS
                    } else {
                        FRAME_COALESCE_NORMAL_MICROS
                    });
                    let settle_ms = if fast_mode {
                        SETTLE_FAST_MILLIS
                    } else {
                        SETTLE_NORMAL_MILLIS
                    };
                    let retry_limit = if fast_mode {
                        RETRY_LIMIT_FAST
                    } else {
                        RETRY_LIMIT_NORMAL
                    };
                    let sweep_limit = if fast_mode {
                        SWEEPS_FAST
                    } else {
                        SWEEPS_NORMAL
                    };

                    if let Some(last_presented_at) = last_presented {
                        let elapsed = last_presented_at.elapsed();
                        if elapsed < frame_budget {
                            cx.background_executor().timer(frame_budget - elapsed).await;
                        }
                    }

                    if process_exit_status.is_some()
                        && this
                            .update(cx, |pane: &mut TerminalPane, _cx| {
                                // Save session to RAMdisk before exit
                                pane.save_session_to_ramdisk();
                                pane.pending_process_exit_status = process_exit_status;
                                if pane.auto_close {
                                    if let Some(pane_id) = pane.auto_close_pane_id {
                                        if let Some(tx) = crate::app::pane_auto_close_sender() {
                                            let _ = tx.send(pane_id);
                                        }
                                    }
                                }
                            })
                            .is_err()
                    {
                        break;
                    }

                    let mut should_notify = false;
                    let mut settled = false;

                    // Phase 1: Poll until the first change is detected (or we
                    // exhaust the retry budget).
                    for attempt in 0..retry_limit {
                        let changed = match this.update(cx, |pane: &mut TerminalPane, _cx| {
                            pane.refresh_snapshot(false)
                        }) {
                            Ok(changed) => changed,
                            Err(_) => {
                                settled = true;
                                break;
                            }
                        };
                        should_notify |= changed;

                        if attempt + 1 == retry_limit {
                            settled = true;
                            break;
                        }

                        if should_notify {
                            break;
                        }

                        cx.background_executor()
                            .timer(Duration::from_millis(settle_ms))
                            .await;
                    }

                    // Phase 2: After the first change, do additional sweeps to
                    // capture the full VT burst.  In fast mode we do more sweeps
                    // with shorter gaps to drain sustained output quickly.
                    if should_notify && !settled {
                        for _sweep in 0..sweep_limit {
                            cx.background_executor()
                                .timer(Duration::from_millis(settle_ms))
                                .await;

                            let sweep_changed = this
                                .update(cx, |pane: &mut TerminalPane, _cx| {
                                    pane.refresh_snapshot(false)
                                })
                                .unwrap_or(false);
                            should_notify |= sweep_changed;

                            let still_pending = this
                                .update(cx, |pane: &mut TerminalPane, _cx| {
                                    pane.surface.has_pending_data()
                                })
                                .unwrap_or(false);
                            if !still_pending && !sweep_changed {
                                break;
                            }
                        }
                    }

                    if this
                        .update(cx, |pane: &mut TerminalPane, _cx| {
                            pane.surface.finish_output_event();
                        })
                        .is_err()
                    {
                        break;
                    }

                    // --- Update adaptive pacing counters ----------------------
                    if should_notify {
                        consecutive_busy = consecutive_busy.saturating_add(1);
                        consecutive_idle = 0;
                    } else {
                        consecutive_idle = consecutive_idle.saturating_add(1);
                        if consecutive_idle >= FAST_PACING_EXIT {
                            consecutive_busy = 0;
                        }
                    }

                    if settled && !should_notify {
                        continue;
                    }

                    if should_notify
                        && this
                            .update(cx, |_pane: &mut TerminalPane, cx| {
                                cx.notify();
                            })
                            .is_err()
                    {
                        break;
                    }
                    if should_notify {
                        last_presented = Some(Instant::now());
                    }
                }
            },
        )
        .detach();

        let font_family = SharedString::from(settings.font_family.clone());
        let (measured_w, measured_h) =
            measure_cell_dimensions(cx, font_family.as_ref(), settings.font_size);
        log::info!(
            "Terminal cell: width={:.2}px height={:.2}px (fallback w={:.1} h={:.1})",
            measured_w,
            measured_h,
            CELL_WIDTH_FALLBACK,
            CELL_HEIGHT_FALLBACK,
        );

        #[cfg(feature = "ghostty_vt")]
        {
            if settings.gpu_acceleration {
                let initial_w = (settings.cols as f32 * measured_w + HORIZONTAL_TEXT_PADDING * 2.0)
                    .ceil() as u32;
                let initial_h = (settings.rows as f32 * measured_h).ceil() as u32;
                let gpu_init_width = initial_w.max(64);
                let gpu_init_height = initial_h.max(64);
                let this = cx.entity().downgrade();
                cx.spawn(async move |_, cx: &mut AsyncApp| {
                    let gpu_result = cx
                        .background_executor()
                        .spawn(async move {
                            GpuTerminalState::new(
                                gpu_init_width,
                                gpu_init_height,
                                settings.font_size,
                            )
                        })
                        .await;

                    let _ = this.update(cx, |pane: &mut TerminalPane, cx| {
                        match gpu_result {
                            Some(state) => {
                                log::info!(
                                    "DX12 GPU terminal renderer active — bypassing GPUI text shaping"
                                );
                                pane.gpu_state = Some(Arc::new(Mutex::new(state)));
                            }
                            None => {
                                let (stage, hr) = ghostty_vt::gpu_renderer_last_init_error();
                                let stage_name = ghostty_vt::gpu_init_stage_name(stage);
                                log::warn!(
                                    "DX12 GPU renderer unavailable — falling back to GPUI text shaping \
                                     (failed at stage {}={}, HRESULT=0x{:08X})",
                                    stage, stage_name, hr as u32
                                );
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            } else {
                log::info!("DX12 GPU terminal renderer disabled; using GPUI text shaping.");
            }
        }

        let blink_entity = cx.entity().downgrade();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                if blink_entity
                    .update(cx, |pane: &mut TerminalPane, cx| {
                        if matches!(pane.state, TerminalState::Running)
                            && pane.cursor_blink
                            && pane.snapshot.cursor_visible
                        {
                            pane.cursor_blink_pending = true;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            surface,
            focus_handle,
            input_suppressed: settings.input_suppressed.clone(),
            state: TerminalState::Pending,
            snapshot: TerminalSnapshot::new(settings.rows),
            cell_width: measured_w,
            cell_height: measured_h,
            font_family,
            font_size: settings.font_size,
            cursor_blink: settings.cursor_blink,
            copy_on_select: settings.copy_on_select,
            gpu_acceleration: settings.gpu_acceleration,
            fg_color: settings.fg_color,
            bg_color: settings.bg_color,
            background_image_path: settings.background_image_path.clone(),
            background_image_opacity: settings.background_image_opacity,
            global_hotkeys: settings.global_hotkeys.clone(),
            blink_started_at: Instant::now(),
            term_cols: settings.cols,
            term_rows: settings.rows,
            last_width: 0.0,
            last_height: 0.0,
            last_bounds: None,
            selection_anchor: None,
            selection_head: None,
            is_selecting: false,
            ime_composing: false,
            wheel_scroll_line_remainder: 0.0,
            recent_user_inputs: VecDeque::new(),
            pending_input: Vec::new(),
            pending_process_exit_status: None,
            auto_close: false,
            auto_close_pane_id: None,
            cursor_blink_pending: false,
            glyph_cache: Default::default(),
            #[cfg(feature = "ghostty_vt")]
            gpu_state: None,
            #[cfg(not(feature = "ghostty_vt"))]
            row_generations: vec![0; settings.rows as usize],
            _focus_in_sub: None,
            context_menu_visible: false,
            context_menu_position: Point::default(),
            scroll_offset: 0,
            scrollbar_dragging: false,
            max_scrollback_lines: settings.scrollback_lines,
            initial_working_directory: saved_working_directory,
            last_session_save: Instant::now(),
        }
    }

    /// Save current session state to RAMdisk for fast persistence.
    /// No-op if ZWG_RAMDISK is not set.
    fn save_session_to_ramdisk(&self) {
        let snapshot = SessionSnapshot {
            cwd: self.initial_working_directory.clone().unwrap_or_default(),
            cols: self.term_cols,
            rows: self.term_rows,
            scroll_offset: self.scroll_offset,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        snapshot.save_to_ramdisk();
    }

    /// Periodic session save: called from render(), saves at most once per SESSION_SAVE_INTERVAL.
    fn maybe_save_session(&mut self) {
        if self.last_session_save.elapsed() >= SESSION_SAVE_INTERVAL {
            self.save_session_to_ramdisk();
            self.last_session_save = Instant::now();
        }
    }

    fn cursor_blink_visible(&self) -> bool {
        if !self.cursor_blink {
            return true;
        }

        (self.blink_started_at.elapsed().as_millis() / 500).is_multiple_of(2)
    }

    #[cfg(feature = "ghostty_vt")]
    fn recreate_gpu_state(&mut self, cx: &mut Context<Self>) {
        if !self.gpu_acceleration {
            self.gpu_state = None;
            return;
        }

        let width = ((self.term_cols as f32 * self.cell_width) + HORIZONTAL_TEXT_PADDING * 2.0)
            .ceil() as u32;
        let height = (self.term_rows as f32 * self.cell_height).ceil() as u32;
        let width = width.max(64);
        let height = height.max(64);
        let font_size = self.font_size;
        let this = cx.entity().downgrade();
        self.gpu_state = None;
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let gpu_result = cx
                .background_executor()
                .spawn(async move { GpuTerminalState::new(width, height, font_size) })
                .await;

            let _ = this.update(cx, |pane: &mut TerminalPane, cx| {
                if let Some(state) = gpu_result {
                    pane.gpu_state = Some(Arc::new(Mutex::new(state)));
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(not(feature = "ghostty_vt"))]
    fn recreate_gpu_state(&mut self, _cx: &mut Context<Self>) {}

    pub fn take_process_exit_status(&mut self) -> Option<i32> {
        self.pending_process_exit_status.take()
    }

    /// Mark this pane for auto-close when its process exits.
    pub fn set_auto_close(&mut self, pane_id: u32) {
        self.auto_close = true;
        self.auto_close_pane_id = Some(pane_id);
    }

    pub fn clear_history(&mut self) {
        self.surface.clear_history();
        self.snapshot = TerminalSnapshot::new(self.term_rows);
        self.selection_anchor = None;
        self.selection_head = None;
        self.is_selecting = false;
        self.pending_process_exit_status = None;
    }

    /// Persist glyph cache keys to RAMdisk for fast startup next time.
    /// Call on app shutdown or periodically.
    #[cfg(feature = "ghostty_vt")]
    pub fn save_glyph_cache(&self) {
        super::grid_renderer::save_glyph_keys_to_ramdisk(&self.glyph_cache.read());
    }

    // ── IPC helpers ────────────────────────────────────────────────

    /// Write raw bytes to the PTY (used by send-keys IPC command)
    pub fn write_to_pty(&self, data: &[u8]) -> anyhow::Result<()> {
        self.surface
            .write_input(data)
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("PTY write failed: {}", e))
    }

    /// Capture the current visible screen content as a string
    pub fn capture_screen(&self) -> String {
        let backend = self.surface.backend.lock();
        let rows = backend.rows;
        let mut content = String::new();
        for row in 0..rows {
            content.push_str(&backend.row_text(row));
            content.push('\n');
        }
        content
    }

    /// Get current terminal dimensions (cols, rows)
    pub fn terminal_size(&self) -> (u16, u16) {
        (self.term_cols, self.term_rows)
    }

    pub fn update_settings(&mut self, settings: &TerminalSettings, cx: &mut Context<Self>) {
        let next_font_family = SharedString::from(settings.font_family.clone());
        let font_changed = self.font_family != next_font_family
            || (self.font_size - settings.font_size).abs() > f32::EPSILON;
        let colors_changed =
            self.fg_color != settings.fg_color || self.bg_color != settings.bg_color;
        let blink_changed = self.cursor_blink != settings.cursor_blink;
        let background_image_changed = self.background_image_path != settings.background_image_path
            || (self.background_image_opacity - settings.background_image_opacity).abs()
                > f32::EPSILON;

        self.input_suppressed = settings.input_suppressed.clone();
        if self.input_suppressed.load(Ordering::Relaxed) {
            self.ime_composing = false;
        }
        self.font_family = next_font_family;
        self.font_size = settings.font_size;
        self.cursor_blink = settings.cursor_blink;
        self.copy_on_select = settings.copy_on_select;
        self.gpu_acceleration = settings.gpu_acceleration;
        self.fg_color = settings.fg_color;
        self.bg_color = settings.bg_color;
        self.background_image_path = settings.background_image_path.clone();
        self.background_image_opacity = settings.background_image_opacity;
        self.global_hotkeys = settings.global_hotkeys.clone();
        self.blink_started_at = Instant::now();
        self.surface
            .set_default_colors(self.fg_color, self.bg_color);

        if font_changed {
            let (cell_width, cell_height) =
                measure_cell_dimensions(cx, self.font_family.as_ref(), self.font_size);
            self.cell_width = cell_width;
            self.cell_height = cell_height;
            #[cfg(feature = "ghostty_vt")]
            {
                super::grid_renderer::save_glyph_keys_to_ramdisk(&self.glyph_cache.read());
                self.glyph_cache.write().clear();
            }
            self.recreate_gpu_state(cx);
            if self.last_width > 0.0 && self.last_height > 0.0 {
                let _ = self.handle_resize(self.last_width, self.last_height);
            }
        }

        if font_changed || colors_changed || blink_changed || background_image_changed {
            self.refresh_snapshot(true);
            cx.notify();
        }
    }

    fn render_background_image(&self) -> Option<AnyElement> {
        let image_path = self.background_image_path.as_ref()?;
        if image_path.trim().is_empty() || self.background_image_opacity <= 0.0 {
            return None;
        }

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .overflow_hidden()
                .child(
                    img(PathBuf::from(image_path))
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .opacity(self.background_image_opacity),
                )
                .into_any_element(),
        )
    }

    /// Recalculate terminal grid size from pixel dimensions
    fn handle_resize(&mut self, width_px: f32, height_px: f32) -> bool {
        if (width_px - self.last_width).abs() < 2.0 && (height_px - self.last_height).abs() < 2.0 {
            return false;
        }
        self.last_width = width_px;
        self.last_height = height_px;

        let content_width = (width_px - HORIZONTAL_TEXT_PADDING * 2.0).max(self.cell_width);
        let new_cols = (content_width / self.cell_width).floor().max(1.0) as u16;
        let new_rows = (height_px / self.cell_height).floor().max(1.0) as u16;

        if new_cols != self.term_cols || new_rows != self.term_rows {
            self.term_cols = new_cols;
            self.term_rows = new_rows;
            self.surface.resize(new_cols, new_rows);
            self.snapshot.resize(new_rows);
            // Save glyph keys before clearing on resize
            #[cfg(feature = "ghostty_vt")]
            {
                super::grid_renderer::save_glyph_keys_to_ramdisk(&self.glyph_cache.read());
                self.glyph_cache.write().clear();
            }
            #[cfg(not(feature = "ghostty_vt"))]
            self.row_generations.resize(new_rows as usize, 0);
            return true;
        }
        false
    }

    fn refresh_snapshot(&mut self, force_full: bool) -> bool {
        // Content refresh invalidates cursor-blink-only state
        self.cursor_blink_pending = false;
        self.snapshot.damaged_rows.clear();
        #[cfg(feature = "ghostty_vt")]
        let (rows, cursor_x, cursor_y, cursor_visible, row_updates, scrolled) = {
            let mut backend = self.surface.backend.lock();
            let rows = backend.rows;
            let pos = backend.cursor_position();
            let cursor_visible = backend.cursor_visible();
            let scroll_delta = if force_full {
                0
            } else {
                backend.terminal.take_viewport_scroll_delta()
            };
            let scrolled = scroll_delta != 0;
            let dirty_rows = viewport_rows_to_refresh(
                rows,
                force_full,
                scroll_delta,
                match backend.terminal.take_dirty_viewport_rows(rows) {
                    Ok(dirty_rows) => Some(dirty_rows),
                    Err(err) => {
                        log::debug!("Failed to collect dirty terminal rows: {}", err);
                        None
                    }
                },
            );
            let row_updates = dirty_rows
                .into_iter()
                .map(|row| GhosttyRowUpdate {
                    row,
                    text: backend.row_text(row),
                    style_runs: backend.row_style_runs(row),
                })
                .collect::<Vec<_>>();
            (
                rows,
                pos.0.saturating_sub(1),
                pos.1.saturating_sub(1),
                cursor_visible,
                row_updates,
                scrolled,
            )
        };

        #[cfg(not(feature = "ghostty_vt"))]
        let (rows, cursor_x, cursor_y, cursor_visible, row_updates) = {
            let backend = self.surface.backend.lock();
            let rows = backend.rows;
            let (cursor_x, cursor_y) = backend.cursor_position();
            let cursor_visible = backend.cursor_visible();
            let dirty_rows: Vec<u16> = if force_full {
                (0..rows).collect()
            } else {
                let mut dirty_rows = Vec::new();
                for row in 0..rows as usize {
                    let next_generation = backend
                        .screen
                        .row_generations
                        .get(row)
                        .copied()
                        .unwrap_or(0);
                    if self.row_generations.get(row).copied().unwrap_or(0) != next_generation {
                        dirty_rows.push(row as u16);
                    }
                }
                dirty_rows
            };
            let row_updates = dirty_rows
                .into_iter()
                .map(|row| {
                    let index = row as usize;
                    FallbackRowUpdate {
                        row,
                        text: backend.row_text(row),
                        generation: backend
                            .screen
                            .row_generations
                            .get(index)
                            .copied()
                            .unwrap_or(0),
                    }
                })
                .collect::<Vec<_>>();
            (rows, cursor_x, cursor_y, cursor_visible, row_updates)
        };

        let cursor_changed = self.snapshot.cursor_x != cursor_x
            || self.snapshot.cursor_y != cursor_y
            || self.snapshot.cursor_visible != cursor_visible;
        let mut changed = cursor_changed;
        let mut content_changed = false;

        if self.snapshot.rows.len() != rows as usize {
            self.snapshot.resize(rows);
            changed = true;
            content_changed = true;
        }

        #[cfg(feature = "ghostty_vt")]
        for row_update in row_updates {
            let index = row_update.row as usize;
            if index >= self.snapshot.rows.len() {
                continue;
            }

            let cached_row = &mut self.snapshot.rows[index];
            // Force full redraw when:
            //  - explicit force_full (resize, config change)
            //  - viewport scrolled (row indices shifted, cache is stale)
            //  - row text or style_runs actually changed — this catches
            //    residual glyphs from async-parsed partial VT responses
            //    (e.g. PSReadLine prediction ghost chars after Backspace)
            //    without falsely signalling the settle loop like a
            //    blanket cursor-row force_full would.
            let text_changed = cached_row.text.as_ref() != row_update.text.as_str()
                || cached_row.style_runs != row_update.style_runs;
            let row_changed = apply_ghostty_row_update(
                cached_row,
                row_update,
                self.term_cols,
                self.fg_color,
                self.bg_color,
                force_full || scrolled || text_changed,
            );
            changed |= row_changed;
            if row_changed {
                self.snapshot.damaged_rows.push(index as u16);
                content_changed = true;
            }
        }

        #[cfg(not(feature = "ghostty_vt"))]
        for row_update in row_updates {
            let index = row_update.row as usize;
            if index >= self.snapshot.rows.len() {
                continue;
            }

            let cached_row = &mut self.snapshot.rows[index];
            let row_changed = cached_row.text.as_ref() != row_update.text.as_str()
                || self.row_generations.get(index).copied().unwrap_or(0) != row_update.generation;
            cached_row.text = SharedString::from(row_update.text);
            self.row_generations[index] = row_update.generation;
            changed |= row_changed;
            if row_changed {
                self.snapshot.damaged_rows.push(index as u16);
                content_changed = true;
            }
        }

        self.snapshot.cursor_x = cursor_x;
        self.snapshot.cursor_y = cursor_y;
        self.snapshot.cursor_visible = cursor_visible;
        if content_changed {
            self.snapshot.content_revision = self.snapshot.content_revision.wrapping_add(1);
        }
        changed
    }

    /// Create a full-size IME registration layer matching terminal bounds.
    fn ime_canvas(&self, cx: &mut Context<Self>) -> Div {
        let entity = cx.entity().clone();
        let focus = self.focus_handle.clone();
        let suppressed = self.input_suppressed.clone();
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let _ = entity.update(cx, |pane, _cx| {
                            pane.last_bounds = Some(bounds);
                        });
                        // Don't register IME handler when input is suppressed
                        // (e.g., group editor overlay is open)
                        if !suppressed.load(Ordering::Relaxed) {
                            log::trace!("IME_TERM register_input handler");
                            let handler = ElementInputHandler::new(bounds, entity.clone());
                            window.handle_input(&focus, handler, cx);
                        }
                    },
                )
                .size_full(),
            )
    }

    /// Render the "Connecting..." placeholder (with key buffering support)
    fn render_pending(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal-pane")
            .relative()
            .size_full()
            .bg(rgb(self.bg_color))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_right_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_drop(cx.listener(Self::on_external_paths_drop))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(SUBTEXT0))
                            .child("Starting shell..."),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(SURFACE0))
                            .child("Initializing ConPTY"),
                    ),
            )
            .child(self.ime_canvas(cx))
    }

    /// Render the error state
    fn render_failed(&self, error: &str) -> impl IntoElement {
        div()
            .id("terminal-pane")
            .size_full()
            .bg(rgb(self.bg_color))
            .track_focus(&self.focus_handle)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(RED))
                            .child("Failed to start shell"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(SUBTEXT0))
                            .child(error.to_string()),
                    ),
            )
    }

    /// Render the running terminal using canvas paint API for pixel-perfect grid.
    fn render_running(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Periodic session save to RAMdisk (every 30s, driven by frame loop)
        self.maybe_save_session();

        self.flush_ime_endcomposition_queue();

        let (layout_width, layout_height) =
            terminal_layout_size(window.viewport_size(), self.last_bounds);
        if self.handle_resize(layout_width, layout_height) {
            self.cursor_blink_pending = false;
            self.refresh_snapshot(true);
        }

        // When the render was triggered only by cursor blink, skip the full
        // snapshot refresh — the content hasn't changed, only cursor visibility.
        if self.cursor_blink_pending {
            self.cursor_blink_pending = false;
            // No refresh_snapshot needed — cursor_blink_visible() handles it below
        }

        let selection = self.selection_range();
        let render_snapshot = {
            let mut snap = self.snapshot.clone();
            snap.cursor_visible &= self.cursor_blink_visible();
            Arc::new(snap)
        };

        let config = GridRendererConfig {
            cell_width: self.cell_width,
            cell_height: self.cell_height,
            font_size: self.font_size,
            horizontal_text_padding: HORIZONTAL_TEXT_PADDING,
            term_cols: self.term_cols,
            fg_color: self.fg_color,
            bg_color: self.bg_color,
            cached_font: font(self.font_family.clone()),
        };

        // Choose rendering path: DX12 native swapchain when possible, otherwise GPUI text shaping.
        #[cfg(feature = "ghostty_vt")]
        let terminal_element: AnyElement = if let Some(ref gpu) = self.gpu_state {
            let cursor = if render_snapshot.cursor_visible {
                render_snapshot
                    .rows
                    .get(render_snapshot.cursor_y as usize)
                    .map(|row| {
                        let (col, width) = resolve_cursor_cell(
                            render_snapshot.cursor_x,
                            &row.cells,
                            self.term_cols,
                        );
                        CursorOverlay {
                            row: render_snapshot.cursor_y,
                            col,
                            width: width as f32,
                        }
                    })
            } else {
                None
            };
            #[cfg(target_os = "windows")]
            {
                if snapshot_can_present_natively(&render_snapshot) {
                    gpu_terminal_canvas(
                        Arc::clone(&render_snapshot),
                        cursor,
                        selection,
                        config,
                        Arc::clone(gpu),
                        self.glyph_cache.clone(),
                    )
                    .into_any_element()
                } else {
                    gpu.lock().hide_native_presenter();
                    terminal_canvas(
                        Arc::clone(&render_snapshot),
                        selection,
                        config,
                        self.glyph_cache.clone(),
                    )
                    .into_any_element()
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                terminal_canvas(
                    Arc::clone(&render_snapshot),
                    selection,
                    config,
                    self.glyph_cache.clone(),
                )
                .into_any_element()
            }
        } else {
            terminal_canvas(
                Arc::clone(&render_snapshot),
                selection,
                config,
                self.glyph_cache.clone(),
            )
            .into_any_element()
        };
        #[cfg(not(feature = "ghostty_vt"))]
        let terminal_element: AnyElement = terminal_canvas(
            Arc::clone(&render_snapshot),
            selection,
            config,
            self.glyph_cache.clone(),
        )
        .into_any_element();

        let mut pane = div()
            .image_cache(retain_all("terminal-background-image-cache"))
            .id("terminal-pane")
            .relative()
            .size_full()
            .bg(rgb(self.bg_color))
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_right_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_drop(cx.listener(Self::on_external_paths_drop));

        if let Some(background) = self.render_background_image() {
            pane = pane.child(background);
        }

        // Keep the IME registration canvas behind the terminal surface so the
        // pane itself remains the mouse event owner for drag selection/copy.
        pane = pane.child(self.ime_canvas(cx)).child(terminal_element);

        // Scrollbar overlay (right edge, semi-transparent)
        if self.scroll_offset > 0 {
            let scrollbar_element = self.render_scrollbar();
            pane = pane.child(scrollbar_element);
        }

        // Context menu overlay
        if self.context_menu_visible {
            pane = pane.child(self.render_context_menu(cx));
        }

        pane
    }

    /// Render a vertical scrollbar as an absolutely-positioned overlay on the right edge.
    fn render_scrollbar(&self) -> Div {
        let visible_rows = self.term_rows as f32;
        let total_lines = (self.scroll_offset as f32 + visible_rows).max(visible_rows + 1.0);
        let track_h = self.term_rows as f32 * self.cell_height;
        let thumb_ratio = (visible_rows / total_lines).clamp(0.05, 1.0);
        let thumb_h = (thumb_ratio * track_h).max(20.0);

        let scrollable = total_lines - visible_rows;
        let scroll_ratio = if scrollable > 0.0 {
            1.0 - (self.scroll_offset as f32 / scrollable)
        } else {
            1.0
        };
        let thumb_top = scroll_ratio * (track_h - thumb_h);

        div()
            .absolute()
            .right_0()
            .top_0()
            .bottom_0()
            .w(px(8.0))
            .bg(rgba(0x3132444du32))
            .child(
                div()
                    .absolute()
                    .right_0()
                    .w(px(8.0))
                    .top(px(thumb_top))
                    .h(px(thumb_h))
                    .rounded(px(4.0))
                    .bg(rgba(0x585b70ccu32)),
            )
    }

    /// Apply scrollbar drag: compute new scroll_offset from mouse Y position
    fn apply_scrollbar_drag(&mut self, mouse_pos: Point<Pixels>, bounds: Bounds<Pixels>) {
        let click_y = mouse_pos.y - bounds.top();
        let track_h: f32 = bounds.size.height.into();
        if track_h <= 0.0 {
            return;
        }
        let ratio = (f32::from(click_y) / track_h).clamp(0.0, 1.0);
        // ratio 0.0 = top (oldest = max offset), 1.0 = bottom (newest = 0)
        let max_offset = self.max_scrollback_lines as i64;
        let new_offset = ((1.0 - ratio) * max_offset as f32).round() as i64;
        let new_offset = new_offset.clamp(0, max_offset);
        if new_offset != self.scroll_offset {
            let delta = new_offset - self.scroll_offset;
            self.surface.scroll_viewport(delta as i32);
            self.scroll_offset = new_offset;
            self.refresh_snapshot(false);
        }
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Lazy-init focus-in listener to reset stale IME state
        if self._focus_in_sub.is_none() {
            let sub = cx.on_focus_in(&self.focus_handle, window, |pane: &mut TerminalPane, _window, _cx| {
                IME_VK_PROCESSKEY.store(false, Ordering::Release);
                pane.ime_composing = false;
                #[cfg(target_os = "windows")]
                {
                    // Drain stale composition queue
                    let _ = take_ime_endcomposition_texts();
                }
            });
            self._focus_in_sub = Some(sub);
        }
        match &self.state {
            TerminalState::Pending => self.render_pending(cx).into_any_element(),
            TerminalState::Failed(err) => {
                let err = err.clone();
                self.render_failed(&err).into_any_element()
            }
            TerminalState::Running => self.render_running(window, cx).into_any_element(),
        }
    }
}

impl TerminalPane {
    fn selection_range(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        selection_range_from_points(self.selection_anchor, self.selection_head)
    }

    fn clear_selection(&mut self) -> bool {
        let had_selection =
            self.selection_anchor.is_some() || self.selection_head.is_some() || self.is_selecting;
        self.selection_anchor = None;
        self.selection_head = None;
        self.is_selecting = false;
        had_selection
    }

    fn set_selection_state(
        &mut self,
        selection_anchor: Option<SelectionPoint>,
        selection_head: Option<SelectionPoint>,
        is_selecting: bool,
    ) -> bool {
        let changed = self.selection_anchor != selection_anchor
            || self.selection_head != selection_head
            || self.is_selecting != is_selecting;
        self.selection_anchor = selection_anchor;
        self.selection_head = selection_head;
        self.is_selecting = is_selecting;
        changed
    }

    fn mouse_to_selection_point(&self, position: Point<Pixels>) -> Option<SelectionPoint> {
        let bounds = self.last_bounds?;
        let row_count = self.snapshot.rows.len();
        if row_count == 0 {
            return None;
        }

        // Clamp to terminal bounds. `Bounds::localize` uses strict `contains` and can drop
        // borderline coordinates, which would stall or cancel drag selection.
        let px = |p: Pixels| f32::from(p);
        let x = px(position.x).clamp(px(bounds.left()), px(bounds.right())) - px(bounds.origin.x);
        let y = px(position.y).clamp(px(bounds.top()), px(bounds.bottom())) - px(bounds.origin.y);
        let row = (y.max(0.0) / self.cell_height)
            .floor()
            .clamp(0.0, (row_count.saturating_sub(1)) as f32) as u16;
        let col = ((x - HORIZONTAL_TEXT_PADDING).max(0.0) / self.cell_width).floor() as u16;

        Some(SelectionPoint {
            row,
            col: col.min(self.term_cols),
        })
    }

    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        let mut lines = Vec::new();

        for row in start.row..=end.row {
            let row_text = self.snapshot.rows.get(row as usize)?.text.as_ref();
            let text = if start.row == end.row {
                slice_text_by_cols(row_text, start.col as usize, end.col as usize)
            } else if row == start.row {
                slice_text_by_cols(row_text, start.col as usize, usize::MAX)
            } else if row == end.row {
                slice_text_by_cols(row_text, 0, end.col as usize)
            } else {
                row_text.to_string()
            };
            lines.push(text);
        }

        Some(lines.join("\n"))
    }

    fn copy_selection_to_clipboard(&self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.selected_text() else {
            return false;
        };
        if text.is_empty() {
            return false;
        }

        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// Copy the current line (where cursor is) to clipboard
    fn copy_current_line_to_clipboard(&self, cx: &mut Context<Self>) {
        let row = self.snapshot.cursor_y as usize;
        if let Some(row_data) = self.snapshot.rows.get(row) {
            let text = row_data.text.trim_end().to_string();
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    /// Reset terminal: clear screen and scrollback
    fn reset_terminal(&mut self) {
        // Send "reset" escape sequence: clear screen + home cursor + clear scrollback
        self.write_terminal_bytes(b"\x1b[2J\x1b[H\x1b[3J");
    }

    // ── Context menu rendering ───────────────────────────────────────────

    fn render_context_menu(&self, cx: &mut Context<Self>) -> Div {
        let menu_x = f32::from(self.context_menu_position.x);
        let menu_y = f32::from(self.context_menu_position.y);
        // Offset from last_bounds origin so the menu is positioned relative to pane
        let (offset_x, offset_y) = if let Some(bounds) = self.last_bounds {
            (
                menu_x - f32::from(bounds.origin.x),
                menu_y - f32::from(bounds.origin.y),
            )
        } else {
            (menu_x, menu_y)
        };

        div()
            .absolute()
            .top(px(offset_y))
            .left(px(offset_x))
            .bg(rgb(0x313244))
            .border_1()
            .border_color(rgb(0x45475a))
            .rounded(px(6.0))
            .shadow_md()
            .py(px(4.0))
            .min_w(px(220.0))
            .font_family("Cascadia Code")
            .text_size(px(12.0))
            .text_color(rgb(0xcdd6f4))
            // Clipboard group
            .child(Self::menu_item("コピーする", "(c)", {
                let listener = cx.listener(|this, _: &MouseDownEvent, _win, cx| {
                    this.context_menu_visible = false;
                    let _ = this.copy_selection_to_clipboard(cx);
                    cx.notify();
                });
                listener
            }))
            .child(Self::menu_item("1行コピーする", "(L)", {
                let listener = cx.listener(|this, _: &MouseDownEvent, _win, cx| {
                    this.context_menu_visible = false;
                    this.copy_current_line_to_clipboard(cx);
                    cx.notify();
                });
                listener
            }))
            .child(Self::menu_item("ペーストする", "(C-y)", {
                let listener = cx.listener(|this, _: &MouseDownEvent, _win, cx| {
                    this.context_menu_visible = false;
                    let _ = this.paste_from_clipboard(cx);
                    cx.notify();
                });
                listener
            }))
            // Separator
            .child(Self::menu_separator())
            // Pane split group
            .child(Self::menu_item("ペインを横に増やす", "(h)", {
                let listener = cx.listener(|this, _: &MouseDownEvent, win, cx| {
                    this.context_menu_visible = false;
                    win.dispatch_action(Box::new(crate::SplitRight), cx);
                    cx.notify();
                });
                listener
            }))
            .child(Self::menu_item("ペインを縦に増やす", "(v)", {
                let listener = cx.listener(|this, _: &MouseDownEvent, win, cx| {
                    this.context_menu_visible = false;
                    win.dispatch_action(Box::new(crate::SplitDown), cx);
                    cx.notify();
                });
                listener
            }))
            // Separator
            .child(Self::menu_separator())
            // Pane management group
            .child(Self::menu_item("ペインを閉じる", "(X)", {
                let listener = cx.listener(|this, _: &MouseDownEvent, win, cx| {
                    this.context_menu_visible = false;
                    win.dispatch_action(Box::new(crate::ClosePane), cx);
                    cx.notify();
                });
                listener
            }))
            .child(Self::menu_item("ペインを初期化する", "(R)", {
                let listener = cx.listener(|this, _: &MouseDownEvent, _win, cx| {
                    this.context_menu_visible = false;
                    this.reset_terminal();
                    cx.notify();
                });
                listener
            }))
            .child(Self::menu_item("ペインを最大化する", "(z)", {
                let listener = cx.listener(|this, _: &MouseDownEvent, win, cx| {
                    this.context_menu_visible = false;
                    win.dispatch_action(Box::new(crate::MaximizePane), cx);
                    cx.notify();
                });
                listener
            }))
    }

    fn menu_item(
        label: &str,
        shortcut: &str,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Div {
        div()
            .px(px(12.0))
            .py(px(6.0))
            .mx(px(4.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(|e| e.bg(rgb(0x45475a)))
            .flex()
            .items_center()
            .justify_between()
            .on_mouse_down(MouseButton::Left, on_click)
            .child(
                div()
                    .text_size(px(12.0))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0x6c7086))
                    .child(shortcut.to_string()),
            )
    }

    fn menu_separator() -> Div {
        div()
            .my(px(4.0))
            .mx(px(8.0))
            .h(px(1.0))
            .bg(rgb(0x45475a))
    }

    fn write_terminal_bytes(&mut self, data: &[u8]) {
        match self.state {
            TerminalState::Running => {
                // Snap to bottom on user input
                if self.scroll_offset != 0 {
                    self.surface.scroll_viewport_bottom();
                    self.scroll_offset = 0;
                }
                let _ = self.surface.write_input(data);
            }
            TerminalState::Pending => {
                // Limit buffered input to prevent unbounded memory growth
                const MAX_PENDING_INPUT: usize = 64 * 1024;
                if self.pending_input.len() + data.len() <= MAX_PENDING_INPUT {
                    self.pending_input.extend_from_slice(data);
                }
            }
            TerminalState::Failed(_) => {}
        }
    }

    fn should_drop_duplicate_user_input(&mut self, source: UserInputSource, data: &[u8]) -> bool {
        let now = Instant::now();
        // Prune entries older than the cross-route window
        let cutoff = Duration::from_millis(CROSS_ROUTE_DUPLICATE_WINDOW_MS);
        while self
            .recent_user_inputs
            .front()
            .is_some_and(|(_, _, at)| now.duration_since(*at) > cutoff)
        {
            self.recent_user_inputs.pop_front();
        }

        // Check against ALL recent inputs, not just the last one.
        // This prevents delayed ImeEndComposition flushes from escaping
        // duplicate detection when subsequent keystrokes have updated the history.
        let duplicate = self
            .recent_user_inputs
            .iter()
            .any(|(prev_source, prev_data, at)| {
                if prev_data != data {
                    return false;
                }

                let elapsed = now.duration_since(*at);
                if *prev_source != source {
                    return elapsed <= Duration::from_millis(CROSS_ROUTE_DUPLICATE_WINDOW_MS);
                }

                source.is_commit_source()
                    && elapsed <= Duration::from_millis(SAME_ROUTE_COMMIT_DUPLICATE_WINDOW_MS)
            });

        // Only record non-duplicate entries to prevent cascading false positives
        // where a dropped ImeEndComposition entry causes the next legitimate
        // TextCommit of the same character to be falsely detected as cross-route duplicate.
        if !duplicate {
            const MAX_RECENT_ENTRIES: usize = 32;
            if self.recent_user_inputs.len() >= MAX_RECENT_ENTRIES {
                self.recent_user_inputs.pop_front();
            }
            self.recent_user_inputs
                .push_back((source, data.to_vec(), now));
        }

        if duplicate && terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM dropped duplicate source={:?} bytes={:?}",
                source as u8,
                data
            );
        }

        duplicate
    }

    fn write_user_input_bytes(&mut self, source: UserInputSource, data: &[u8]) {
        if data.is_empty() || self.should_drop_duplicate_user_input(source, data) {
            return;
        }

        self.write_terminal_bytes(data);
    }

    fn write_user_input_text(&mut self, source: UserInputSource, text: &str) {
        if text.is_empty() {
            return;
        }

        let bytes = text_to_terminal_bytes(text, self.surface.win32_input_mode());
        self.write_user_input_bytes(source, &bytes);
    }

    fn flush_ime_endcomposition_queue(&mut self) {
        #[cfg(target_os = "windows")]
        {
            let texts = take_ime_endcomposition_texts_for_terminal(
                self.input_suppressed.load(Ordering::Relaxed),
            );
            if !texts.is_empty() && terminal_ime_trace_enabled() {
                log::debug!(
                    "IME_TERM flush queue: {} item(s) from IME end composition",
                    texts.len()
                );
            }

            for text in texts {
                if text.is_empty() {
                    continue;
                }

                if terminal_ime_trace_enabled() {
                    log::debug!(
                        "IME_TERM flush endcomposition text={:?} bytes={:?}",
                        text,
                        text.as_bytes()
                    );
                }

                // Forward to PTY — duplicates are suppressed by should_drop_duplicate_user_input
                self.write_user_input_text(UserInputSource::ImeEndComposition, &text);
            }
        }

        #[cfg(not(target_os = "windows"))]
        {}
    }

    pub fn paste_text(&mut self, text: &str) -> bool {
        let bytes = normalize_terminal_newlines(text);
        if bytes.is_empty() {
            return false;
        }

        self.clear_selection();
        self.write_terminal_bytes(&bytes);
        true
    }

    fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        // Try GPUI clipboard first, fall back to native Windows API
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .or_else(|| read_clipboard_native_text());

        let Some(text) = text else {
            return false;
        };

        self.paste_text(&text)
    }

    fn on_external_paths_drop(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = format_dropped_paths(paths);
        if text.is_empty() {
            return;
        }

        window.focus(&self.focus_handle);
        let selection_changed = self.clear_selection();
        self.write_terminal_bytes(text.as_bytes());
        if selection_changed {
            cx.notify();
        }
    }

    /// Convert a keystroke to bytes for PTY. Returns None if the key should not
    /// be sent (e.g., modifier-only keys).
    #[cfg(test)]
    fn keystroke_to_bytes(ks: &Keystroke) -> Option<Vec<u8>> {
        Self::keystroke_to_bytes_with_win32_mode(ks, false)
    }

    fn keystroke_to_bytes_with_win32_mode(
        ks: &Keystroke,
        win32_input_mode: bool,
    ) -> Option<Vec<u8>> {
        // IME VK_PROCESSKEY: hook already called TranslateMessage.
        // Only defer when this event is still part of IME processing.
        let ime_processkey_pending = IME_VK_PROCESSKEY.swap(false, Ordering::AcqRel);
        if should_defer_keystroke_to_ime(ks, ime_processkey_pending) {
            return None;
        }

        // key_char: actual character from ToUnicode (shift/layout aware)
        if let Some(ref kc) = ks.key_char {
            if !kc.is_empty() {
                if win32_input_mode && !ks.modifiers.alt && !ks.modifiers.control {
                    return Some(text_to_terminal_bytes(kc, true));
                }
                if ks.modifiers.alt {
                    let mut buf = vec![0x1b];
                    buf.extend_from_slice(kc.as_bytes());
                    return Some(buf);
                }
                return Some(kc.as_bytes().to_vec());
            }
        }

        let key_str = ks.key.as_ref();
        let ctrl = ks.modifiers.control;
        let alt = ks.modifiers.alt;
        let shift = ks.modifiers.shift;

        match key_str {
            // Keep Enter as CR, but let Shift+Enter emulate Ctrl+J/LF so
            // multiline terminal prompts such as Codex CLI can insert a newline.
            "enter" if shift && !ctrl && !alt => Some(vec![b'\n']),
            "enter" => Some(b"\r".to_vec()),
            "backspace" => Some(vec![0x7f]),
            "tab" => Some(b"\t".to_vec()),
            "escape" => Some(vec![0x1b]),
            "space" => {
                if win32_input_mode && !alt && !ctrl {
                    Some(text_to_terminal_bytes(" ", true))
                } else if alt {
                    Some(vec![0x1b, b' '])
                } else {
                    Some(vec![b' '])
                }
            }
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
            "f1" => Some(b"\x1bOP".to_vec()),
            "f2" => Some(b"\x1bOQ".to_vec()),
            "f3" => Some(b"\x1bOR".to_vec()),
            "f4" => Some(b"\x1bOS".to_vec()),
            "f5" => Some(b"\x1b[15~".to_vec()),
            "f6" => Some(b"\x1b[17~".to_vec()),
            "f7" => Some(b"\x1b[18~".to_vec()),
            "f8" => Some(b"\x1b[19~".to_vec()),
            "f9" => Some(b"\x1b[20~".to_vec()),
            "f10" => Some(b"\x1b[21~".to_vec()),
            "f11" => Some(b"\x1b[23~".to_vec()),
            "f12" => Some(b"\x1b[24~".to_vec()),
            _ => {
                if ctrl && key_str.len() == 1 {
                    let ch = key_str.chars().next().unwrap();
                    if ch.is_ascii_lowercase() {
                        Some(vec![ch as u8 - b'a' + 1])
                    } else {
                        None
                    }
                } else if key_str.len() == 1 {
                    // Fallback: single-char key without key_char (shouldn't normally happen)
                    Some(key_str.as_bytes().to_vec())
                } else {
                    None
                }
            }
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        log_terminal_ime_keystroke("on_key_down", &event.keystroke, "terminal pane keydown");

        // Close context menu on Escape
        if self.context_menu_visible {
            self.context_menu_visible = false;
            cx.notify();
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                return;
            }
        }

        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }

        if self
            .global_hotkeys
            .iter()
            .any(|hotkey| crate::app::hotkey_matches(event, hotkey))
        {
            return;
        }

        let key: &str = event.keystroke.key.as_ref();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control && !modifiers.alt && key == "c" && self.copy_selection_to_clipboard(cx)
        {
            cx.stop_propagation();
            return;
        }
        if modifiers.control && !modifiers.alt && key == "v" {
            if self.paste_from_clipboard(cx) {
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }

        if should_route_keystroke_via_text_input(&event.keystroke) {
            // Consume IME flag to prevent residual state if replace_text_in_range
            // is not subsequently called by gpui (safety net).
            IME_VK_PROCESSKEY.store(false, Ordering::Release);
            // Snap to bottom so the prompt stays visible during IME input.
            if self.scroll_offset != 0 {
                self.surface.scroll_viewport_bottom();
                self.scroll_offset = 0;
                cx.notify();
            } else if self.clear_selection() {
                cx.notify();
            }
            return;
        }

        // Snap to bottom on any keypress so the user always sees the prompt.
        if self.scroll_offset != 0 {
            self.surface.scroll_viewport_bottom();
            self.scroll_offset = 0;
        }

        let bytes = match Self::keystroke_to_bytes_with_win32_mode(
            &event.keystroke,
            self.surface.win32_input_mode(),
        ) {
            Some(b) => b,
            None => {
                // IME key or unknown — stop propagation to prevent gpui's
                // broken TranslateMessage from interfering with IME.
                log_terminal_ime_keystroke(
                    "on_key_down",
                    &event.keystroke,
                    "keystroke_to_bytes:none -> stop propagation",
                );
                cx.stop_propagation();
                cx.notify();
                return;
            }
        };

        match self.state {
            TerminalState::Running => {
                let selection_changed = self.clear_selection();
                self.write_user_input_bytes(UserInputSource::KeyDown, &bytes);
                if selection_changed {
                    cx.notify();
                }
                cx.stop_propagation();
            }
            TerminalState::Pending => {
                // Buffer input while PTY is connecting (capped)
                self.write_terminal_bytes(&bytes);
                let selection_changed = self.clear_selection();
                if selection_changed {
                    cx.notify();
                }
                cx.stop_propagation();
            }
            TerminalState::Failed(_) => {}
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        // Dismiss context menu on left-click
        if self.context_menu_visible {
            self.context_menu_visible = false;
            cx.notify();
            return;
        }
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }
        // Check if click is on scrollbar track (right 8px)
        if self.scroll_offset > 0 {
            if let Some(bounds) = self.last_bounds {
                let click_x = event.position.x - bounds.left();
                let sb_width = px(8.0);
                if click_x >= bounds.size.width - sb_width {
                    self.scrollbar_dragging = true;
                    self.apply_scrollbar_drag(event.position, bounds);
                    cx.notify();
                    return;
                }
            }
        }
        if let Some(point) = self.mouse_to_selection_point(event.position) {
            if self.set_selection_state(Some(point), Some(point), true) {
                cx.notify();
            }
        } else {
            if self.clear_selection() {
                cx.notify();
            }
        }
    }

    fn on_mouse_right_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }

        // Show context menu at click position
        self.context_menu_visible = true;
        self.context_menu_position = event.position;
        cx.notify();
        cx.stop_propagation();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Scrollbar drag
        if self.scrollbar_dragging && event.dragging() {
            if let Some(bounds) = self.last_bounds {
                self.apply_scrollbar_drag(event.position, bounds);
                cx.notify();
            }
            return;
        }
        if !self.is_selecting {
            return;
        }
        if let Some(point) = self.mouse_to_selection_point(event.position) {
            if self.selection_head != Some(point) {
                self.selection_head = Some(point);
                cx.notify();
            }
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.scrollbar_dragging {
            self.scrollbar_dragging = false;
            cx.notify();
            return;
        }
        if !self.is_selecting {
            return;
        }
        let prev_anchor = self.selection_anchor;
        let prev_head = self.selection_head;
        let was_selecting = self.is_selecting;
        if let Some(point) = self.mouse_to_selection_point(event.position) {
            self.selection_head = Some(point);
        }
        self.is_selecting = false;
        let should_notify = if self.selection_range().is_none() {
            self.clear_selection()
        } else {
            prev_anchor != self.selection_anchor
                || prev_head != self.selection_head
                || was_selecting != self.is_selecting
        };
        if should_notify {
            if self.copy_on_select && self.selection_range().is_some() {
                let _ = self.copy_selection_to_clipboard(cx);
            }
            cx.notify();
        }
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }
        if !matches!(self.state, TerminalState::Running) {
            return;
        }

        let delta_y = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => {
                let pixels: f32 = delta.y.into();
                pixels
            }
        };
        if delta_y == 0.0 {
            return;
        }

        let line_delta = scroll_lines_from_wheel_delta(
            event.delta,
            self.cell_height,
            &mut self.wheel_scroll_line_remainder,
        );
        if line_delta == 0 {
            cx.stop_propagation();
            return;
        }

        if self.surface.scroll_viewport(line_delta) {
            // Track scroll offset for scrollbar (positive = scrolled up into history)
            self.scroll_offset = (self.scroll_offset + line_delta as i64)
                .max(0)
                .min(self.max_scrollback_lines as i64);
            if self.refresh_snapshot(false) {
                cx.notify();
            }
            cx.stop_propagation();
        }
    }
}

impl Focusable for TerminalPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn selection_range_from_points(
    start: Option<SelectionPoint>,
    end: Option<SelectionPoint>,
) -> Option<(SelectionPoint, SelectionPoint)> {
    let start = start?;
    let end = end?;
    if start == end {
        None
    } else if start <= end {
        Some((start, end))
    } else {
        Some((end, start))
    }
}

#[cfg(test)]
fn should_copy_selection_on_right_click(
    selection: Option<(SelectionPoint, SelectionPoint)>,
) -> bool {
    selection.is_some()
}

// ── IME support via EntityInputHandler ──────────────────────────────
// Receives committed text from Windows IME (Japanese/Chinese/Korean)
// via gpui's WM_IME_COMPOSITION → replace_text_in_range path.

impl EntityInputHandler for TerminalPane {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        if terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM text_for_range called range={:?} adjusted={:?}",
                _range,
                _adjusted
            );
        }
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if terminal_ime_trace_enabled() {
            log::debug!("IME_TERM selected_text_range called ignore={}", _ignore);
        }
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        if terminal_ime_trace_enabled() {
            log::debug!("IME_TERM marked_text_range called");
        }
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if terminal_ime_trace_enabled() {
            log::debug!("IME_TERM unmark_text called");
        }
        self.ime_composing = false;
    }

    /// Committed text from IME → send to PTY
    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }
        if text.is_empty() {
            self.ime_composing = false;
            log::debug!("IME_TERM replace_text_in_range empty text; composing cleared");
            return;
        }
        if terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM replace_text_in_range state={:?} range={:?} text={:?} bytes={:?}",
                self.state,
                _range,
                text,
                text.as_bytes()
            );
        }
        let was_composing = self.ime_composing;
        let should_forward = should_forward_replace_text_to_terminal(text, was_composing);
        self.ime_composing = false;
        IME_VK_PROCESSKEY.store(false, Ordering::Release);

        // Flush any pending IME queue entries before writing the TextCommit,
        // so that duplicate detection can properly suppress the queue duplicate.
        self.flush_ime_endcomposition_queue();

        if terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM replace_text_in_range forward={} composing_before_reset={}",
                should_forward,
                was_composing
            );
        }
        if !should_forward {
            return;
        }
        self.write_user_input_text(UserInputSource::TextCommit, text);
        // Trigger re-render so terminal displays the PTY echo promptly
        cx.notify();
    }

    /// Preedit (composing) text from IME — currently not displayed,
    /// but accepting the call prevents gpui from dropping the composition.
    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _new_text: &str,
        _new_selected: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM replace_and_mark_text_in_range composing={} len={}",
                !_new_text.is_empty(),
                _new_text.len()
            );
        }
        if self.input_suppressed.load(Ordering::Relaxed) {
            return;
        }
        // Snap to bottom during IME preedit so the prompt stays visible.
        if self.scroll_offset != 0 {
            self.surface.scroll_viewport_bottom();
            self.scroll_offset = 0;
        }
        let was_composing = self.ime_composing;
        self.ime_composing = !_new_text.is_empty();
        IME_VK_PROCESSKEY.store(false, Ordering::Release);
        // Notify so that the next render cycle flushes any pending IME queue
        if was_composing != self.ime_composing {
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM bounds_for_range called range={:?} composing={}",
                _range,
                self.ime_composing
            );
        }
        // Always return cursor position so the IME candidate window
        // tracks the correct location. Returning None causes the window
        // to appear at screen origin (bottom-left).
        Some(Bounds::new(
            point(
                element_bounds.origin.x
                    + px(HORIZONTAL_TEXT_PADDING + self.snapshot.cursor_x as f32 * self.cell_width),
                element_bounds.origin.y + px(self.snapshot.cursor_y as f32 * self.cell_height),
            ),
            size(px(self.cell_width), px(self.cell_height)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        if terminal_ime_trace_enabled() {
            log::debug!(
                "IME_TERM character_index_for_point called point={:?}",
                _point
            );
        }
        None
    }
}

impl EventEmitter<()> for TerminalPane {}

fn slice_text_by_cols(text: &str, start_col: usize, end_col: usize) -> String {
    let start_idx = col_to_char_index(text, start_col);
    let end_idx = if end_col == usize::MAX {
        text.chars().count()
    } else {
        col_to_char_index(text, end_col)
    };

    text.chars()
        .skip(start_idx)
        .take(end_idx.saturating_sub(start_idx))
        .collect()
}

#[cfg(test)]
mod snapshot_tests {
    use super::{
        TerminalPane, scroll_lines_from_wheel_delta, selection_range_from_points,
        should_copy_selection_on_right_click, should_defer_keystroke_to_ime_with_state,
        should_forward_replace_text_to_terminal, should_route_keystroke_via_text_input_with_state,
        terminal_layout_size, viewport_rows_to_refresh,
    };
    use gpui::{Bounds, Keystroke, Modifiers, ScrollDelta, point, px, size};

    #[cfg(target_os = "windows")]
    fn push_ime_endcomposition_test_text(text: &str) {
        match super::IME_COMPOSITION_RESULT_QUEUE.lock() {
            Ok(mut queue) => queue.push_back(text.to_string()),
            Err(err) => err.into_inner().push_back(text.to_string()),
        }
    }

    #[test]
    fn viewport_rows_to_refresh_returns_dirty_rows_without_scroll() {
        assert_eq!(
            viewport_rows_to_refresh(4, false, 0, Some(vec![1, 3])),
            vec![1, 3]
        );
    }

    #[test]
    fn viewport_rows_to_refresh_forces_full_refresh_when_viewport_scrolled() {
        assert_eq!(
            viewport_rows_to_refresh(4, false, 1, Some(vec![3])),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn viewport_rows_to_refresh_falls_back_to_full_refresh_on_dirty_row_failure() {
        assert_eq!(viewport_rows_to_refresh(3, false, 0, None), vec![0, 1, 2]);
    }

    #[test]
    fn should_defer_keystroke_to_ime_handles_ascii_and_non_ascii() {
        let process_key = Keystroke {
            modifiers: Modifiers::default(),
            key: "ime-process".into(),
            key_char: None,
        };
        let ascii_key = Keystroke {
            modifiers: Modifiers::default(),
            key: "a".into(),
            key_char: Some("a".into()),
        };
        let non_ascii_key = Keystroke {
            modifiers: Modifiers::default(),
            key: "a".into(),
            key_char: Some("\u{3042}".into()), // あ
        };

        // pending=false → always false
        assert!(!should_defer_keystroke_to_ime_with_state(
            &process_key,
            false,
            true
        ));
        assert!(!should_defer_keystroke_to_ime_with_state(
            &ascii_key, false, true
        ));
        assert!(!should_defer_keystroke_to_ime_with_state(
            &non_ascii_key,
            false,
            true
        ));

        // pending=true, no key_char → defer (IME still processing)
        assert!(should_defer_keystroke_to_ime_with_state(
            &process_key,
            true,
            true
        ));

        // pending=true, ASCII key_char → defer (romaji input like a, k, i)
        assert!(should_defer_keystroke_to_ime_with_state(
            &ascii_key, true, true
        ));

        // pending=true, non-ASCII key_char → don't defer (committed char like あ)
        assert!(!should_defer_keystroke_to_ime_with_state(
            &non_ascii_key,
            true,
            true
        ));
    }

    #[test]
    fn should_defer_keystroke_to_ime_allows_ascii_when_ime_is_already_in_alnum_mode() {
        let ascii_key = Keystroke {
            modifiers: Modifiers::default(),
            key: "a".into(),
            key_char: Some("a".into()),
        };

        assert!(!should_defer_keystroke_to_ime_with_state(
            &ascii_key, true, false
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shift_enter_maps_to_line_feed_for_multiline_prompts() {
        let shift_enter = Keystroke {
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            key: "enter".into(),
            key_char: None,
        };

        assert_eq!(
            TerminalPane::keystroke_to_bytes(&shift_enter),
            Some(vec![b'\n'])
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn enter_without_plain_shift_modifier_stays_carriage_return() {
        let plain_enter = Keystroke {
            modifiers: Modifiers::default(),
            key: "enter".into(),
            key_char: None,
        };
        let ctrl_shift_enter = Keystroke {
            modifiers: Modifiers {
                control: true,
                shift: true,
                ..Modifiers::default()
            },
            key: "enter".into(),
            key_char: None,
        };
        let alt_shift_enter = Keystroke {
            modifiers: Modifiers {
                alt: true,
                shift: true,
                ..Modifiers::default()
            },
            key: "enter".into(),
            key_char: None,
        };

        assert_eq!(
            TerminalPane::keystroke_to_bytes(&plain_enter),
            Some(vec![b'\r'])
        );
        assert_eq!(
            TerminalPane::keystroke_to_bytes(&ctrl_shift_enter),
            Some(vec![b'\r'])
        );
        assert_eq!(
            TerminalPane::keystroke_to_bytes(&alt_shift_enter),
            Some(vec![b'\r'])
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn printable_windows_keys_stay_on_terminal_input_path_outside_ime_processing() {
        let printable_ascii = Keystroke {
            modifiers: Modifiers::default(),
            key: "a".into(),
            key_char: Some("a".into()),
        };
        let printable_non_ascii = Keystroke {
            modifiers: Modifiers::default(),
            key: "ime".into(),
            key_char: Some("あ".into()),
        };
        let ctrl = Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            key: "c".into(),
            key_char: Some("c".into()),
        };

        assert!(!should_route_keystroke_via_text_input_with_state(
            &printable_ascii,
            false,
            true
        ));
        assert!(!should_route_keystroke_via_text_input_with_state(
            &printable_non_ascii,
            false,
            true
        ));
        assert!(!should_route_keystroke_via_text_input_with_state(
            &ctrl, false, true
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn printable_windows_keys_route_via_text_input_during_ime_processing() {
        let printable_non_ascii = Keystroke {
            modifiers: Modifiers::default(),
            key: "ime".into(),
            key_char: Some("あ".into()),
        };

        assert!(should_route_keystroke_via_text_input_with_state(
            &printable_non_ascii,
            true,
            true
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn printable_windows_symbols_stay_on_keydown_path_when_ime_is_in_alnum_mode() {
        let middle_dot = Keystroke {
            modifiers: Modifiers::default(),
            key: "ime".into(),
            key_char: Some("・".into()),
        };
        let right_arrow = Keystroke {
            modifiers: Modifiers::default(),
            key: "ime".into(),
            key_char: Some("→".into()),
        };

        assert!(!should_route_keystroke_via_text_input_with_state(
            &middle_dot,
            true,
            false
        ));
        assert!(!should_route_keystroke_via_text_input_with_state(
            &right_arrow,
            true,
            false
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn printable_windows_symbols_encode_as_win32_input_records_when_mode_is_enabled() {
        let middle_dot = Keystroke {
            modifiers: Modifiers::default(),
            key: "ime".into(),
            key_char: Some("・".into()),
        };

        let encoded = TerminalPane::keystroke_to_bytes_with_win32_mode(&middle_dot, true)
            .expect("unicode key should encode in win32 input mode");

        assert_eq!(
            String::from_utf8(encoded).expect("win32 input sequence should be utf8"),
            "\x1b[0;0;12539;1;0;1_\x1b[0;0;12539;0;0;1_"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn replace_text_in_range_forwards_text_commits_on_windows() {
        assert!(should_forward_replace_text_to_terminal("a", false));
        assert!(should_forward_replace_text_to_terminal("あ", false));
        assert!(should_forward_replace_text_to_terminal("あ", true));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn queued_endcomposition_text_is_dropped_while_terminal_input_is_suppressed() {
        push_ime_endcomposition_test_text("日本語");

        assert!(super::take_ime_endcomposition_texts_for_terminal(true).is_empty());
        assert!(super::take_ime_endcomposition_texts().is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn queued_endcomposition_text_is_forwarded_when_terminal_input_is_active() {
        push_ime_endcomposition_test_text("日本語");

        assert_eq!(
            super::take_ime_endcomposition_texts_for_terminal(false),
            vec!["日本語".to_string()]
        );
        assert!(super::take_ime_endcomposition_texts().is_empty());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn replace_text_in_range_keeps_text_commits_on_non_windows() {
        assert!(should_forward_replace_text_to_terminal("a", false));
        assert!(should_forward_replace_text_to_terminal("あ", false));
    }

    #[test]
    fn scroll_lines_from_wheel_delta_preserves_line_steps() {
        let mut remainder = 0.0;
        assert_eq!(
            scroll_lines_from_wheel_delta(
                ScrollDelta::Lines(point(0.0, 3.0)),
                20.0,
                &mut remainder
            ),
            -3
        );
        assert_eq!(remainder, 0.0);
    }

    #[test]
    fn scroll_lines_from_wheel_delta_accumulates_fractional_pixels() {
        let mut remainder = 0.0;
        assert_eq!(
            scroll_lines_from_wheel_delta(
                ScrollDelta::Pixels(point(px(0.0), px(9.0))),
                20.0,
                &mut remainder
            ),
            0
        );
        assert!(remainder < -0.4 && remainder > -0.5);

        assert_eq!(
            scroll_lines_from_wheel_delta(
                ScrollDelta::Pixels(point(px(0.0), px(11.0))),
                20.0,
                &mut remainder
            ),
            -1
        );
        assert_eq!(remainder, 0.0);
    }

    #[test]
    fn terminal_layout_size_prefers_last_terminal_bounds() {
        let (width, height) = terminal_layout_size(
            size(px(1200.0), px(900.0)),
            Some(Bounds::new(
                point(px(10.0), px(20.0)),
                size(px(840.0), px(640.0)),
            )),
        );

        assert_eq!(width, 840.0);
        assert_eq!(height, 640.0);
    }

    #[test]
    fn terminal_layout_size_falls_back_to_viewport_minus_chrome() {
        let (width, height) = terminal_layout_size(size(px(1200.0), px(900.0)), None);

        assert_eq!(width, 1200.0);
        assert_eq!(height, 840.0);
    }

    #[test]
    fn selection_range_from_points_orders_reversed_points() {
        assert_eq!(
            selection_range_from_points(
                Some(super::SelectionPoint { row: 2, col: 8 }),
                Some(super::SelectionPoint { row: 0, col: 3 })
            ),
            Some((
                super::SelectionPoint { row: 0, col: 3 },
                super::SelectionPoint { row: 2, col: 8 }
            ))
        );
    }

    #[test]
    fn selection_range_from_points_ignores_single_cell_selection() {
        assert_eq!(
            selection_range_from_points(
                Some(super::SelectionPoint { row: 1, col: 4 }),
                Some(super::SelectionPoint { row: 1, col: 4 })
            ),
            None
        );
    }

    #[test]
    fn right_click_copy_requires_existing_selection() {
        assert!(!should_copy_selection_on_right_click(None));
        assert!(should_copy_selection_on_right_click(Some((
            super::SelectionPoint { row: 0, col: 0 },
            super::SelectionPoint { row: 0, col: 5 }
        ))));
    }
}

#[cfg(all(test, feature = "ghostty_vt"))]
mod tests {
    use super::*;
    use crate::terminal::grid_renderer::CachedTerminalRow;
    use crate::terminal::{DEFAULT_BG, DEFAULT_FG};

    fn style_run(start_col: u16, end_col: u16, bg_rgb: u32) -> ghostty_vt::StyleRun {
        ghostty_vt::StyleRun {
            start_col,
            end_col,
            fg: ghostty_vt::Rgb {
                r: ((DEFAULT_FG >> 16) & 0xFF) as u8,
                g: ((DEFAULT_FG >> 8) & 0xFF) as u8,
                b: (DEFAULT_FG & 0xFF) as u8,
            },
            bg: ghostty_vt::Rgb {
                r: ((bg_rgb >> 16) & 0xFF) as u8,
                g: ((bg_rgb >> 8) & 0xFF) as u8,
                b: (bg_rgb & 0xFF) as u8,
            },
            flags: 0,
        }
    }

    #[::core::prelude::v1::test]
    fn apply_ghostty_row_update_skips_redraw_for_identical_row() {
        let style_runs = vec![style_run(1, 2, DEFAULT_BG)];
        let cells = grid_cells_from_parts("ab", &style_runs, 4, DEFAULT_FG, DEFAULT_BG);
        let mut row = CachedTerminalRow {
            text: SharedString::from("ab"),
            style_runs: style_runs.clone(),
            cells: cells.clone(),
            glyph_instances: super::super::grid_renderer::glyph_instances_from_cells(&cells, 0),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let changed = apply_ghostty_row_update(
            &mut row,
            GhosttyRowUpdate {
                row: 0,
                text: "ab".into(),
                style_runs,
            },
            4,
            DEFAULT_FG,
            DEFAULT_BG,
            false,
        );

        assert!(!changed);
        assert!(row.damage_spans.is_empty());
        assert!(row.damaged_glyph_instances.is_empty());
    }

    #[::core::prelude::v1::test]
    fn apply_ghostty_row_update_marks_style_only_changes_as_damage() {
        let previous_style_runs = vec![style_run(2, 2, DEFAULT_BG)];
        let cells = grid_cells_from_parts("ab", &previous_style_runs, 4, DEFAULT_FG, DEFAULT_BG);
        let mut row = CachedTerminalRow {
            text: SharedString::from("ab"),
            style_runs: previous_style_runs,
            cells: cells.clone(),
            glyph_instances: super::super::grid_renderer::glyph_instances_from_cells(&cells, 0),
            damage_spans: Vec::new(),
            damaged_glyph_instances: Vec::new(),
        };

        let changed = apply_ghostty_row_update(
            &mut row,
            GhosttyRowUpdate {
                row: 0,
                text: "ab".into(),
                style_runs: vec![style_run(2, 2, 0x224466)],
            },
            4,
            DEFAULT_FG,
            DEFAULT_BG,
            false,
        );

        assert!(changed);
        assert_eq!(
            row.damage_spans,
            vec![super::super::grid_renderer::DamageSpan {
                start_col: 1,
                end_col: 2,
            }]
        );
    }
}
