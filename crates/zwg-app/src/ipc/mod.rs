//! IPC Server for CLI communication
//!
//! Windows: Named pipes (\\.\pipe\zwg)
//! Fallback: TCP on 127.0.0.1:51985

pub mod bridge;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: u64,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: u64,
    pub success: bool,
    pub data: serde_json::Value,
    pub error: Option<String>,
}

impl IpcResponse {
    pub fn ok(id: u64, data: serde_json::Value) -> Self {
        Self {
            id,
            success: true,
            data,
            error: None,
        }
    }
    pub fn err(id: u64, msg: &str) -> Self {
        Self {
            id,
            success: false,
            data: serde_json::Value::Null,
            error: Some(msg.to_string()),
        }
    }
}

type CommandHandler = Arc<dyn Fn(&IpcRequest) -> IpcResponse + Send + Sync>;

pub struct IpcServer {
    handlers: Arc<Mutex<HashMap<String, CommandHandler>>>,
    running: Arc<Mutex<bool>>,
}

impl IpcServer {
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// Register a command handler
    pub fn on_command<F>(&self, command: &str, handler: F)
    where
        F: Fn(&IpcRequest) -> IpcResponse + Send + Sync + 'static,
    {
        self.handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(command.to_string(), Arc::new(handler));
    }

    /// Start the IPC server in a background thread
    pub fn start(&self) -> anyhow::Result<()> {
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = true;
        let running = self.running.clone();
        let handlers = self.handlers.clone();

        thread::spawn(move || {
            if let Err(e) = Self::run_server(running, handlers) {
                log::error!("IPC server error: {}", e);
            }
        });

        Ok(())
    }

    #[allow(dead_code)]
    pub fn stop(&self) {
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = false;
    }

    // ========== Unix: Domain Socket ==========
    #[cfg(not(windows))]
    fn run_server(
        running: Arc<Mutex<bool>>,
        handlers: Arc<Mutex<HashMap<String, CommandHandler>>>,
    ) -> anyhow::Result<()> {
        use std::os::unix::net::UnixListener;

        let sock_path = Self::socket_path();
        if let Some(parent) = sock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&sock_path);

        let listener = UnixListener::bind(&sock_path)?;
        listener.set_nonblocking(true)?;
        log::info!("IPC server listening on {:?}", sock_path);

        while *running.lock().unwrap_or_else(|e| e.into_inner()) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let handlers = handlers.clone();
                    thread::spawn(move || {
                        Self::handle_connection(BufReader::new(&stream), &stream, &handlers);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    log::error!("IPC accept error: {}", e);
                    break;
                }
            }
        }

        let _ = std::fs::remove_file(&sock_path);
        Ok(())
    }

    // ========== Windows: Named Pipe + TCP fallback ==========
    #[cfg(windows)]
    fn run_server(
        running: Arc<Mutex<bool>>,
        handlers: Arc<Mutex<HashMap<String, CommandHandler>>>,
    ) -> anyhow::Result<()> {
        // Start Named Pipe server in its own thread
        let running_np = running.clone();
        let handlers_np = handlers.clone();
        let np_handle = thread::spawn(move || {
            if let Err(e) = Self::run_named_pipe_server(running_np, handlers_np) {
                log::warn!("Named pipe server error: {}, TCP fallback active", e);
            }
        });

        // Also start TCP fallback for WSL/cross-platform clients
        let running_tcp = running.clone();
        let handlers_tcp = handlers.clone();
        let tcp_handle = thread::spawn(move || {
            if let Err(e) = Self::run_tcp_server(running_tcp, handlers_tcp) {
                log::warn!("TCP fallback server error: {}", e);
            }
        });

        let _ = np_handle.join();
        let _ = tcp_handle.join();
        Ok(())
    }

    #[cfg(windows)]
    fn run_named_pipe_server(
        running: Arc<Mutex<bool>>,
        handlers: Arc<Mutex<HashMap<String, CommandHandler>>>,
    ) -> anyhow::Result<()> {
        use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
            PIPE_TYPE_BYTE, PIPE_WAIT,
        };
        use windows::core::w;

        const PIPE_NAME: &windows::core::PCWSTR = &w!(r"\\.\pipe\zwg");
        const BUFFER_SIZE: u32 = 8192;

        log::info!(r"Named pipe IPC server starting on \\.\pipe\zwg");

        while *running.lock().unwrap_or_else(|e| e.into_inner()) {
            // Create a new pipe instance for each connection
            let pipe = unsafe {
                CreateNamedPipeW(
                    *PIPE_NAME,
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    64, // max instances — supports 8+ concurrent teammate agents
                    BUFFER_SIZE,
                    BUFFER_SIZE,
                    3000, // default timeout ms
                    None,
                )
            };

            if pipe == INVALID_HANDLE_VALUE || pipe.is_invalid() {
                log::error!(
                    "CreateNamedPipeW failed: {}",
                    std::io::Error::last_os_error()
                );
                break;
            }

            // Wait for client connection (blocking)
            let connected = unsafe { ConnectNamedPipe(pipe, None) };
            if connected.is_err() {
                // ERROR_PIPE_CONNECTED means client connected before ConnectNamedPipe
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(535) {
                    log::warn!("ConnectNamedPipe error: {}", err);
                    unsafe { CloseHandle(pipe).ok() };
                    continue;
                }
            }

            let handlers = handlers.clone();
            // HANDLE is not Send (*mut c_void), so pass raw value
            let pipe_raw = pipe.0 as usize;
            thread::spawn(move || {
                use windows::Win32::Foundation::{CloseHandle, HANDLE};
                let pipe = HANDLE(pipe_raw as *mut _);
                Self::handle_named_pipe_connection(pipe, &handlers);
                unsafe {
                    DisconnectNamedPipe(pipe).ok();
                    CloseHandle(pipe).ok();
                }
            });
        }

        Ok(())
    }

    #[cfg(windows)]
    fn handle_named_pipe_connection(
        pipe: windows::Win32::Foundation::HANDLE,
        handlers: &Arc<Mutex<HashMap<String, CommandHandler>>>,
    ) {
        // Wrap the raw HANDLE in a Read/Write adapter so we can use BufReader
        // for proper line-based reading (fixes message fragmentation bug).
        let reader = NamedPipeReader(pipe);
        let writer = NamedPipeWriter(pipe);
        Self::handle_connection(BufReader::new(reader), writer, handlers);
    }

    #[cfg(windows)]
    fn run_tcp_server(
        running: Arc<Mutex<bool>>,
        handlers: Arc<Mutex<HashMap<String, CommandHandler>>>,
    ) -> anyhow::Result<()> {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:51985")?;
        listener.set_nonblocking(true)?;
        log::info!("IPC TCP fallback listening on 127.0.0.1:51985");

        while *running.lock().unwrap_or_else(|e| e.into_inner()) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let handlers = handlers.clone();
                    thread::spawn(move || {
                        Self::handle_connection(BufReader::new(&stream), &stream, &handlers);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    log::error!("TCP fallback accept error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    fn handle_connection<R: BufRead, W: Write>(
        reader: R,
        mut writer: W,
        handlers: &Arc<Mutex<HashMap<String, CommandHandler>>>,
    ) {
        let connection_start = std::time::Instant::now();
        // Max connection lifetime: 10 minutes (prevents leaked/zombie connections)
        const MAX_CONNECTION_LIFETIME: std::time::Duration = std::time::Duration::from_secs(600);

        for line in reader.lines() {
            // Prevent zombie connections from blocking pipe instances
            if connection_start.elapsed() > MAX_CONNECTION_LIFETIME {
                log::warn!("[IPC] connection exceeded max lifetime (10min), closing");
                break;
            }
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    log::warn!("[IPC] connection read error: {}", e);
                    break;
                }
            };

            if line.trim().is_empty() {
                continue;
            }

            let request: IpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let preview: String = line.chars().take(200).collect();
                    log::error!("[IPC] invalid JSON (len={}): {} | data: {}", line.len(), e, preview);
                    let resp = IpcResponse::err(0, &format!("Invalid JSON: {}", e));
                    let _ = writeln!(
                        writer,
                        "{}",
                        serde_json::to_string(&resp).unwrap_or_else(|e| format!(
                            "{{\"error\":\"serialization: {}\"}}",
                            e
                        ))
                    );
                    let _ = writer.flush();
                    continue;
                }
            };

            let response = {
                let handlers = handlers.lock().unwrap_or_else(|e| e.into_inner());
                match handlers.get(&request.command) {
                    Some(h) => h(&request),
                    None => IpcResponse::err(
                        request.id,
                        &format!("Unknown command: {}", request.command),
                    ),
                }
            };

            let _ = writeln!(
                writer,
                "{}",
                serde_json::to_string(&response).unwrap_or_else(|e| format!(
                    "{{\"error\":\"serialization: {}\"}}",
                    e
                ))
            );
            let _ = writer.flush();
        }
    }

    #[cfg(not(windows))]
    pub fn socket_path() -> std::path::PathBuf {
        dirs::runtime_dir()
            .or_else(|| dirs::data_local_dir())
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("zwg")
            .join("zwg.sock")
    }

    /// Return the named pipe path (Windows) or socket path (Unix) as a string
    /// suitable for the $TMUX environment variable.
    pub fn connection_string() -> String {
        #[cfg(windows)]
        {
            r"\\.\pipe\zwg".to_string()
        }
        #[cfg(not(windows))]
        {
            Self::socket_path().to_string_lossy().to_string()
        }
    }
}

// ── Named Pipe Read/Write wrappers (Windows only) ──────────────────────────
//
// These wrap a raw Windows HANDLE in `std::io::Read` / `std::io::Write` so
// that `BufReader::lines()` can be used for proper line-buffered IPC reading,
// identical to the TCP and Unix socket paths.

/// Wrapper to implement `std::io::Read` for a Windows Named Pipe HANDLE.
#[cfg(windows)]
struct NamedPipeReader(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for NamedPipeReader {}

#[cfg(windows)]
impl std::io::Read for NamedPipeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use windows::Win32::Storage::FileSystem::ReadFile;

        let mut bytes_read: u32 = 0;
        let result = unsafe { ReadFile(self.0, Some(buf), Some(&mut bytes_read), None) };

        match result {
            Ok(()) => Ok(bytes_read as usize),
            Err(e) => {
                // ERROR_BROKEN_PIPE (109) means client disconnected — treat as EOF
                let os_err = std::io::Error::last_os_error();
                if os_err.raw_os_error() == Some(109) {
                    Ok(0)
                } else {
                    log::warn!("[IPC] named pipe ReadFile error: {} (win32: {})", os_err, e);
                    Err(os_err)
                }
            }
        }
    }
}

/// Wrapper to implement `std::io::Write` for a Windows Named Pipe HANDLE.
#[cfg(windows)]
struct NamedPipeWriter(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for NamedPipeWriter {}

#[cfg(windows)]
impl std::io::Write for NamedPipeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use windows::Win32::Storage::FileSystem::WriteFile;

        let mut written: u32 = 0;
        let result = unsafe { WriteFile(self.0, Some(buf), Some(&mut written), None) };

        match result {
            Ok(()) => Ok(written as usize),
            Err(_) => Err(std::io::Error::last_os_error()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        use windows::Win32::Storage::FileSystem::FlushFileBuffers;

        unsafe { FlushFileBuffers(self.0) }.map_err(|_| std::io::Error::last_os_error())
    }
}

/// Register default IPC command handlers
pub fn register_default_handlers(server: &IpcServer) {
    server.on_command("ping", |req| {
        IpcResponse::ok(req.id, serde_json::json!({"pong": true}))
    });

    server.on_command("version", |req| {
        IpcResponse::ok(
            req.id,
            serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "name": "zwg"
            }),
        )
    });
}
