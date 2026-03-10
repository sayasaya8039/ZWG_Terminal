//! Terminal surface — manages PTY + terminal backend

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use flume::{Receiver, Sender};
use parking_lot::Mutex;

use super::pty::{ConPtyConfig, PtyPair, spawn_pty};
use super::{DEFAULT_BG, DEFAULT_FG};

/// Events emitted by the terminal
#[derive(Debug)]
pub enum TerminalEvent {
    OutputReceived,
    TitleChanged(String),
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
    }

    impl TerminalBackend {
        pub fn new(cols: u16, rows: u16) -> Self {
            // M4: log error but continue — panic in constructor is unrecoverable
            let mut terminal = match ghostty_vt::Terminal::new(cols, rows) {
                Ok(t) => t,
                Err(e) => {
                    log::error!("Failed to create ghostty terminal: {}. Retrying with defaults.", e);
                    ghostty_vt::Terminal::new(80, 24)
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
            self.terminal
                .dump_viewport_row(row)
                .unwrap_or_default()
        }

        pub fn row_style_runs(&self, row: u16) -> Vec<ghostty_vt::StyleRun> {
            self.terminal
                .dump_viewport_row_style_runs(row)
                .unwrap_or_default()
        }

        pub fn cursor_position(&self) -> (u16, u16) {
            self.terminal.cursor_position().unwrap_or((0, 0))
        }
    }
}

// ── Fallback VT backend (Phase 0) ─────────────────────────────────
#[cfg(not(feature = "ghostty_vt"))]
mod backend {
    use super::*;
    use crate::terminal::vt_parser::VtParser;
    use crate::terminal::ScreenBuffer;

    pub struct TerminalBackend {
        parser: VtParser,
        pub screen: ScreenBuffer,
        pub cols: u16,
        pub rows: u16,
    }

    impl TerminalBackend {
        pub fn new(cols: u16, rows: u16) -> Self {
            Self {
                parser: VtParser::new(),
                screen: ScreenBuffer::new(cols, rows, 10_000),
                cols,
                rows,
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
    }
}

pub use backend::TerminalBackend;

/// Terminal surface: connects PTY ↔ terminal backend
pub struct TerminalSurface {
    pub backend: Arc<Mutex<TerminalBackend>>,
    pub event_rx: Receiver<TerminalEvent>,
    event_tx: Sender<TerminalEvent>,
    pty: Option<Arc<PtyPair>>,
    reader_handle: Option<std::thread::JoinHandle<()>>,
    // H2: stop flag for clean reader thread shutdown
    stop_flag: Arc<AtomicBool>,
}

impl TerminalSurface {
    pub fn new(cols: u16, rows: u16) -> Self {
        // H1: bounded(1) — reader only needs to signal "data available"
        let (event_tx, event_rx) = flume::bounded(4);
        Self {
            backend: Arc::new(Mutex::new(TerminalBackend::new(cols, rows))),
            event_rx,
            event_tx,
            pty: None,
            reader_handle: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn a shell and start reading PTY output
    pub fn spawn(&mut self, shell: &str) -> std::io::Result<()> {
        let (cols, rows) = {
            let b = self.backend.lock();
            (b.cols, b.rows)
        };
        let config = ConPtyConfig {
            shell: shell.to_string(),
            cols,
            rows,
            ..Default::default()
        };

        let pty = Arc::new(spawn_pty(config)?);
        self.pty = Some(pty.clone());

        // Start reader thread
        let reader = pty.reader();
        let backend = self.backend.clone();
        let event_tx = self.event_tx.clone();
        let stop_flag = self.stop_flag.clone();

        let handle = std::thread::Builder::new()
            .name("zwg-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 65536];
                loop {
                    // H2: check stop flag before blocking read
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

                    // Feed PTY output into terminal backend
                    {
                        let mut b = backend.lock();
                        b.feed(&buf[..n]);
                    }

                    // H1: try_send on bounded channel — drop if full (already notified)
                    let _ = event_tx.try_send(TerminalEvent::OutputReceived);
                }

                log::debug!("PTY reader thread exiting");
                // H3/L2: notify process exit
                let _ = event_tx.try_send(TerminalEvent::ProcessExited(0));
            })?;

        self.reader_handle = Some(handle);
        Ok(())
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
        let stop_flag = self.stop_flag.clone();

        let handle = std::thread::Builder::new()
            .name("zwg-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 65536];
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
                    {
                        let mut b = backend.lock();
                        b.feed(&buf[..n]);
                    }
                    let _ = event_tx.try_send(TerminalEvent::OutputReceived);
                }
                log::debug!("PTY reader thread exiting");
                let _ = event_tx.try_send(TerminalEvent::ProcessExited(0));
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

    /// Check if the PTY is connected
    pub fn is_connected(&self) -> bool {
        self.pty.is_some()
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
