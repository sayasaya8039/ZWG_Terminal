//! Terminal surface — manages PTY + screen buffer + VT parser

use std::io::Read;
use std::sync::Arc;

use flume::{Receiver, Sender};
use parking_lot::Mutex;

use super::pty::{ConPtyConfig, PtyPair, spawn_pty};
use super::vt_parser::VtParser;
use super::ScreenBuffer;

/// Events emitted by the terminal
#[derive(Debug)]
pub enum TerminalEvent {
    OutputReceived,
    TitleChanged(String),
    ProcessExited(i32),
}

/// Terminal surface: connects PTY ↔ VT parser ↔ screen buffer
pub struct TerminalSurface {
    pub screen: Arc<Mutex<ScreenBuffer>>,
    pub event_rx: Receiver<TerminalEvent>,
    event_tx: Sender<TerminalEvent>,
    pty: Option<Arc<PtyPair>>,
    parser: Arc<Mutex<VtParser>>,
}

impl TerminalSurface {
    pub fn new(cols: u16, rows: u16) -> Self {
        let (event_tx, event_rx) = flume::unbounded();
        Self {
            screen: Arc::new(Mutex::new(ScreenBuffer::new(cols, rows, 10_000))),
            event_rx,
            event_tx,
            pty: None,
            parser: Arc::new(Mutex::new(VtParser::new())),
        }
    }

    /// Spawn a shell and start reading PTY output
    pub fn spawn(&mut self, shell: &str) -> std::io::Result<()> {
        let config = ConPtyConfig {
            shell: shell.to_string(),
            cols: self.screen.lock().cols,
            rows: self.screen.lock().rows,
            ..Default::default()
        };

        let pty = Arc::new(spawn_pty(config)?);
        self.pty = Some(pty.clone());

        // Start reader thread
        let reader = pty.reader();
        let screen = self.screen.clone();
        let parser = self.parser.clone();
        let event_tx = self.event_tx.clone();

        std::thread::Builder::new()
            .name("zwg-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    let n = {
                        let mut guard = reader.lock();
                        match guard.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        }
                    };

                    // Process through VT parser → screen buffer
                    {
                        let mut p = parser.lock();
                        let mut s = screen.lock();
                        p.process(&buf[..n], &mut s);
                    }

                    let _ = event_tx.try_send(TerminalEvent::OutputReceived);
                }

                let _ = event_tx.try_send(TerminalEvent::ProcessExited(0));
            })?;

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

    /// Resize the terminal
    pub fn resize(&self, cols: u16, rows: u16) {
        self.screen.lock().resize(cols, rows);
        if let Some(ref pty) = self.pty {
            let _ = pty.resize(cols, rows);
        }
    }
}
