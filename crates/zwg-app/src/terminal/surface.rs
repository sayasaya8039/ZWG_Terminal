//! Terminal surface — manages PTY + terminal backend

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use flume::{Receiver, Sender};
use parking_lot::Mutex;

use super::pty::PtyPair;
use super::win32_input::Win32InputModeTracker;
use super::{DEFAULT_BG, DEFAULT_FG};

/// Events emitted by the terminal
#[derive(Debug)]
pub enum TerminalEvent {
    OutputReceived,
    ProcessExited(i32),
}

// ── Ghostty VT backend ────────────────────────────────────────────
#[cfg(feature = "ghostty_vt")]
mod backend {
    use super::*;

    pub struct TerminalBackend {
        pub terminal: ghostty_vt::Terminal,
        pub cols: u16,
        pub rows: u16,
        pub max_scrollback: usize,
        pub default_fg: u32,
        pub default_bg: u32,
    }

    impl TerminalBackend {
        pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Self {
            // M4: log error but continue — panic in constructor is unrecoverable
            let mut terminal =
                match ghostty_vt::Terminal::new_with_scrollback(cols, rows, max_scrollback) {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!(
                            "Failed to create ghostty terminal: {}. Retrying with defaults.",
                            e
                        );
                        ghostty_vt::Terminal::new_with_scrollback(80, 24, max_scrollback)
                            .expect("ghostty terminal creation failed with defaults")
                    }
                };
            terminal.set_default_colors(
                ghostty_vt::Rgb {
                    r: ((DEFAULT_FG >> 16) & 0xFF) as u8,
                    g: ((DEFAULT_FG >> 8) & 0xFF) as u8,
                    b: (DEFAULT_FG & 0xFF) as u8,
                },
                ghostty_vt::Rgb {
                    r: ((DEFAULT_BG >> 16) & 0xFF) as u8,
                    g: ((DEFAULT_BG >> 8) & 0xFF) as u8,
                    b: (DEFAULT_BG & 0xFF) as u8,
                },
            );
            Self {
                terminal,
                cols,
                rows,
                max_scrollback,
                default_fg: DEFAULT_FG,
                default_bg: DEFAULT_BG,
            }
        }

        pub fn feed(&mut self, data: &[u8]) {
            let _ = self.terminal.feed(data);
        }

        pub fn resize(&mut self, cols: u16, rows: u16) {
            self.cols = cols;
            self.rows = rows;
            let _ = self.terminal.resize(cols, rows);
        }

        pub fn row_text(&self, row: u16) -> String {
            self.terminal.dump_viewport_row(row).unwrap_or_default()
        }

        pub fn row_style_runs(&self, row: u16) -> Vec<ghostty_vt::StyleRun> {
            self.terminal
                .dump_viewport_row_style_runs(row)
                .unwrap_or_default()
        }

        pub fn cursor_position(&self) -> (u16, u16) {
            self.terminal.cursor_position().unwrap_or((0, 0))
        }

        pub fn cursor_visible(&self) -> bool {
            self.terminal.cursor_visible()
        }

        pub fn set_default_colors(&mut self, fg_rgb: u32, bg_rgb: u32) {
            self.default_fg = fg_rgb;
            self.default_bg = bg_rgb;
            self.terminal.set_default_colors(
                ghostty_vt::Rgb {
                    r: ((fg_rgb >> 16) & 0xFF) as u8,
                    g: ((fg_rgb >> 8) & 0xFF) as u8,
                    b: (fg_rgb & 0xFF) as u8,
                },
                ghostty_vt::Rgb {
                    r: ((bg_rgb >> 16) & 0xFF) as u8,
                    g: ((bg_rgb >> 8) & 0xFF) as u8,
                    b: (bg_rgb & 0xFF) as u8,
                },
            );
        }

        pub fn scroll_viewport(&mut self, delta_lines: i32) -> bool {
            self.terminal.scroll_viewport(delta_lines).is_ok()
        }

        pub fn clear_history(&mut self) {
            let mut terminal = ghostty_vt::Terminal::new_with_scrollback(
                self.cols,
                self.rows,
                self.max_scrollback,
            )
            .or_else(|_| ghostty_vt::Terminal::new_with_scrollback(80, 24, self.max_scrollback))
            .expect("ghostty terminal recreation failed while clearing history");
            terminal.set_default_colors(
                ghostty_vt::Rgb {
                    r: ((self.default_fg >> 16) & 0xFF) as u8,
                    g: ((self.default_fg >> 8) & 0xFF) as u8,
                    b: (self.default_fg & 0xFF) as u8,
                },
                ghostty_vt::Rgb {
                    r: ((self.default_bg >> 16) & 0xFF) as u8,
                    g: ((self.default_bg >> 8) & 0xFF) as u8,
                    b: (self.default_bg & 0xFF) as u8,
                },
            );
            self.terminal = terminal;
        }

        /// Start async I/O mode (dedicated Zig parser thread).
        pub fn start_async(&mut self) -> bool {
            self.terminal.start_async().is_ok()
        }

        /// Raw terminal handle as usize (for async feeding without Rust mutex).
        pub fn terminal_raw_ptr(&self) -> usize {
            self.terminal.raw_ptr() as usize
        }

        /// Check if the async parser has processed new data since the last call.
        /// Returns false when async I/O is not active.
        pub fn has_new_data(&self) -> bool {
            self.terminal.has_new_data()
        }
    }
}

// ── Fallback VT backend (Phase 0) ─────────────────────────────────
#[cfg(not(feature = "ghostty_vt"))]
mod backend {
    use super::*;
    use crate::terminal::ScreenBuffer;
    use crate::terminal::vt_parser::VtParser;

    pub struct TerminalBackend {
        parser: VtParser,
        pub screen: ScreenBuffer,
        pub cols: u16,
        pub rows: u16,
        pub default_fg: u32,
        pub default_bg: u32,
    }

    impl TerminalBackend {
        pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Self {
            Self {
                parser: VtParser::new(),
                screen: ScreenBuffer::new(cols, rows, max_scrollback),
                cols,
                rows,
                default_fg: DEFAULT_FG,
                default_bg: DEFAULT_BG,
            }
        }

        pub fn feed(&mut self, data: &[u8]) {
            self.parser.process(data, &mut self.screen);
        }

        pub fn resize(&mut self, cols: u16, rows: u16) {
            self.cols = cols;
            self.rows = rows;
            self.screen.resize(cols, rows);
        }

        pub fn row_text(&self, row: u16) -> String {
            self.screen
                .visible_line(row as usize)
                .map(|l| l.text.clone())
                .unwrap_or_default()
        }

        pub fn row_style_runs(&self, _row: u16) -> Vec<()> {
            Vec::new()
        }

        pub fn cursor_position(&self) -> (u16, u16) {
            (self.screen.cursor.x, self.screen.cursor.y)
        }

        pub fn cursor_visible(&self) -> bool {
            self.screen.cursor.visible
        }

        pub fn set_default_colors(&mut self, fg_rgb: u32, bg_rgb: u32) {
            self.default_fg = fg_rgb;
            self.default_bg = bg_rgb;
            self.screen.current_fg = fg_rgb;
            self.screen.current_bg = bg_rgb;
        }

        pub fn scroll_viewport(&mut self, delta_lines: i32) -> bool {
            self.screen.scroll_viewport(delta_lines);
            true
        }

        pub fn clear_history(&mut self) {
            self.screen.clear_history();
        }
    }
}

pub use backend::TerminalBackend;

/// Feed data asynchronously using a raw terminal handle pointer.
/// Non-blocking ring buffer push, bypasses Rust mutex entirely.
#[cfg(feature = "ghostty_vt")]
unsafe fn feed_async_raw(raw_ptr: usize, data: &[u8]) {
    unsafe { ghostty_vt::feed_async_raw(raw_ptr as *mut core::ffi::c_void, data) };
}

/// Terminal surface: connects PTY ↔ terminal backend
pub struct TerminalSurface {
    pub backend: Arc<Mutex<TerminalBackend>>,
    event_rx: Option<Receiver<TerminalEvent>>,
    event_tx: Sender<TerminalEvent>,
    output_event_pending: Arc<AtomicBool>,
    win32_input_mode: Arc<AtomicBool>,
    pty: Option<Arc<PtyPair>>,
    reader_handle: Option<std::thread::JoinHandle<()>>,
    // H2: stop flag for clean reader thread shutdown
    stop_flag: Arc<AtomicBool>,
}

impl TerminalSurface {
    pub fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Self {
        // Perf: bounded(8) — enough headroom for burst; reader coalesces anyway
        let (event_tx, event_rx) = flume::bounded(8);
        Self {
            backend: Arc::new(Mutex::new(TerminalBackend::new(
                cols,
                rows,
                scrollback_lines,
            ))),
            event_rx: Some(event_rx),
            event_tx,
            output_event_pending: Arc::new(AtomicBool::new(false)),
            win32_input_mode: Arc::new(AtomicBool::new(false)),
            pty: None,
            reader_handle: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn take_event_rx(&mut self) -> Receiver<TerminalEvent> {
        self.event_rx
            .take()
            .expect("terminal event receiver already taken")
    }

    pub fn clear_history(&self) {
        self.backend.lock().clear_history();
    }

    pub fn finish_output_event(&self) {
        self.output_event_pending.store(false, Ordering::Release);
    }

    pub fn win32_input_mode(&self) -> bool {
        self.win32_input_mode.load(Ordering::Acquire)
    }

    /// Check if the async parser still has unprocessed data in its ring buffer.
    /// Used by the settle loop to wait for PSReadLine's multi-chunk VT bursts.
    #[cfg(feature = "ghostty_vt")]
    pub fn has_pending_data(&self) -> bool {
        self.backend.lock().has_new_data()
    }

    /// Fallback: no async parser, so never pending.
    #[cfg(not(feature = "ghostty_vt"))]
    pub fn has_pending_data(&self) -> bool {
        false
    }

    /// Write input to the PTY
    pub fn write_input(&self, data: &[u8]) -> std::io::Result<usize> {
        if let Some(ref pty) = self.pty {
            pty.write_input(data)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "PTY not spawned",
            ))
        }
    }

    /// Attach an already-spawned PTY and start the reader thread.
    /// Called from async context after background PTY creation.
    pub fn attach_pty(&mut self, pty: Arc<PtyPair>) -> std::io::Result<()> {
        self.pty = Some(pty.clone());

        // Resize PTY to current backend dimensions
        let (cols, rows) = {
            let b = self.backend.lock();
            (b.cols, b.rows)
        };
        let _ = pty.resize(cols, rows);

        // Start reader thread (same logic as spawn)
        let reader = pty.reader();
        let backend = self.backend.clone();
        let event_tx = self.event_tx.clone();
        let output_event_pending = self.output_event_pending.clone();
        let win32_input_mode = self.win32_input_mode.clone();
        let stop_flag = self.stop_flag.clone();

        // Enable async I/O for attach_pty path
        #[cfg(feature = "ghostty_vt")]
        let async_ptr: Option<usize> = {
            let mut b = backend.lock();
            if b.start_async() {
                log::info!(
                    "Async I/O enabled (attach_pty): PTY reader → ring buffer → parser thread"
                );
                Some(b.terminal_raw_ptr())
            } else {
                None
            }
        };
        #[cfg(not(feature = "ghostty_vt"))]
        let async_ptr: Option<usize> = None;

        let handle = std::thread::Builder::new()
            .name("zwg-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 131_072];
                let mut win32_input_tracker = Win32InputModeTracker::default();
                loop {
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    let n = {
                        let mut guard = reader.lock();
                        match guard.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        }
                    };
                    let win32_mode_active = win32_input_tracker.observe(&buf[..n]);
                    win32_input_mode.store(win32_mode_active, Ordering::Release);
                    if let Some(raw_ptr) = async_ptr {
                        #[cfg(feature = "ghostty_vt")]
                        unsafe {
                            feed_async_raw(raw_ptr, &buf[..n]);
                        }
                    } else {
                        let mut b = backend.lock();
                        b.feed(&buf[..n]);
                    }
                    if output_event_pending
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        if event_tx.try_send(TerminalEvent::OutputReceived).is_err() {
                            output_event_pending.store(false, Ordering::Release);
                        }
                    }
                }
                log::debug!("PTY reader thread exiting");
                output_event_pending.store(false, Ordering::Release);
                win32_input_mode.store(false, Ordering::Release);
                let _ = event_tx.send(TerminalEvent::ProcessExited(0));
            })?;

        self.reader_handle = Some(handle);
        Ok(())
    }

    /// Resize the terminal
    pub fn resize(&self, cols: u16, rows: u16) {
        self.backend.lock().resize(cols, rows);
        if let Some(ref pty) = self.pty {
            let _ = pty.resize(cols, rows);
        }
    }

    pub fn scroll_viewport(&self, delta_lines: i32) -> bool {
        if delta_lines == 0 {
            return false;
        }

        self.backend.lock().scroll_viewport(delta_lines)
    }

    pub fn set_default_colors(&self, fg_rgb: u32, bg_rgb: u32) {
        self.backend.lock().set_default_colors(fg_rgb, bg_rgb);
    }
}

impl Drop for TerminalSurface {
    fn drop(&mut self) {
        // H2: signal reader to stop
        self.stop_flag.store(true, Ordering::Relaxed);

        // Drop PTY first → ClosePseudoConsole → pipe broken → reader gets EOF
        self.pty.take();

        // H2: join reader in a background thread to avoid blocking UI
        if let Some(handle) = self.reader_handle.take() {
            std::thread::Builder::new()
                .name("zwg-pty-cleanup".into())
                .spawn(move || {
                    let _ = handle.join();
                })
                .ok();
        }
    }
}
