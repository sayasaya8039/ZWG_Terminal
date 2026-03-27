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
                    10, // max instances
                    BUFFER_SIZE,
                    BUFFER_SIZE,
                    5000, // default timeout ms
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
        use windows::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile};

        let mut read_buf = [0u8; 8192];
        let mut bytes_read: u32 = 0;

        loop {
            let success =
                unsafe { ReadFile(pipe, Some(&mut read_buf), Some(&mut bytes_read), None) };

            if success.is_err() || bytes_read == 0 {
                break;
            }

            let data = &read_buf[..bytes_read as usize];

            // Process each line (JSON-RPC style)
            for line in data.split(|&b| b == b'\n') {
                let line = line.strip_suffix(&[b'\r']).unwrap_or(line);
                if line.is_empty() {
                    continue;
                }

                let line_str = match std::str::from_utf8(line) {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let request: IpcRequest = match serde_json::from_str(line_str) {
                    Ok(r) => r,
                    Err(e) => {
                        let resp = IpcResponse::err(0, &format!("Invalid JSON: {}", e));
                        let resp_json = format!(
                            "{}\n",
                            serde_json::to_string(&resp).unwrap_or_else(|e| format!(
                                "{{\"error\":\"serialization: {}\"}}",
                                e
                            ))
                        );
                        let resp_bytes = resp_json.as_bytes();
                        let mut written: u32 = 0;
                        unsafe {
                            WriteFile(pipe, Some(resp_bytes), Some(&mut written), None).ok();
                            FlushFileBuffers(pipe).ok();
                        }
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

                let resp_json = format!(
                    "{}\n",
                    serde_json::to_string(&response).unwrap_or_else(|e| format!(
                        "{{\"error\":\"serialization: {}\"}}",
                        e
                    ))
                );
                let resp_bytes = resp_json.as_bytes();
                let mut written: u32 = 0;
                unsafe {
                    WriteFile(pipe, Some(resp_bytes), Some(&mut written), None).ok();
                    FlushFileBuffers(pipe).ok();
                }
            }
        }
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
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            let request: IpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let resp = IpcResponse::err(0, &format!("Invalid JSON: {}", e));
                    let _ = writeln!(
                        writer,
                        "{}",
                        serde_json::to_string(&resp).unwrap_or_else(|e| format!(
                            "{{\"error\":\"serialization: {}\"}}",
                            e
                        ))
                    );
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
