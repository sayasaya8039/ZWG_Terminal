//! Cross-platform PTY implementation
//! Windows: ConPTY (CreatePseudoConsole)

use parking_lot::Mutex;
use std::io::{self, Read, Write};
use std::sync::Arc;

#[derive(Clone)]
pub struct ConPtyConfig {
    pub shell: String,
    pub working_directory: Option<String>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

impl Default for ConPtyConfig {
    fn default() -> Self {
        Self {
            shell: crate::shell::detect_default_shell(),
            working_directory: None,
            env: Vec::new(),
            cols: 80,
            rows: 24,
        }
    }
}

/// Abstraction over a PTY connection
pub struct PtyPair {
    master_read: Arc<Mutex<Box<dyn Read + Send>>>,
    master_write: Arc<Mutex<Box<dyn Write + Send>>>,
    child_pid: u32,
    #[cfg(windows)]
    pseudo_console: Option<PseudoConsoleHandle>,
}

unsafe impl Send for PtyPair {}
unsafe impl Sync for PtyPair {}

impl PtyPair {
    pub fn write_input(&self, data: &[u8]) -> io::Result<usize> {
        self.master_write.lock().write(data)
    }

    pub fn reader(&self) -> Arc<Mutex<Box<dyn Read + Send>>> {
        self.master_read.clone()
    }

    #[allow(dead_code)]
    pub fn child_pid(&self) -> u32 {
        self.child_pid
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        #[cfg(windows)]
        {
            if let Some(ref pc) = self.pseudo_console {
                windows_resize(pc, cols, rows)
            } else {
                Ok(())
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (cols, rows);
            Ok(())
        }
    }
}

// Windows ConPTY
#[cfg(windows)]
struct PseudoConsoleHandle(windows::Win32::System::Console::HPCON);

#[cfg(windows)]
unsafe impl Send for PseudoConsoleHandle {}
#[cfg(windows)]
unsafe impl Sync for PseudoConsoleHandle {}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::fs::File;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::path::Path;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Console::{COORD, CreatePseudoConsole, HPCON};
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW,
        UpdateProcThreadAttribute, CREATE_UNICODE_ENVIRONMENT,
    };
    use windows::core::{PCWSTR, PWSTR};

    fn split_shell_command(shell: &str) -> (String, String) {
        let trimmed = shell.trim();
        if trimmed.is_empty() {
            return ("powershell.exe".to_string(), String::new());
        }

        if let Some(rest) = trimmed.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                let exe = rest[..end].to_string();
                let args = rest[end + 1..].trim().to_string();
                if !exe.is_empty() {
                    return (exe, args);
                }
            }
        }

        if Path::new(trimmed).exists() {
            return (trimmed.to_string(), String::new());
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return ("powershell.exe".to_string(), String::new());
        }

        for i in (1..=parts.len()).rev() {
            let candidate = parts[..i].join(" ");
            if Path::new(&candidate).exists() {
                let args = parts[i..].join(" ");
                return (candidate, args);
            }
        }

        let exe = parts[0].to_string();
        let args = parts[1..].join(" ");
        (exe, args)
    }

    fn quote_cmd(exe: &str) -> String {
        if exe.contains(' ') || exe.contains('\t') {
            format!("\"{}\"", exe.replace('"', "\\\""))
        } else {
            exe.to_string()
        }
    }

    pub fn spawn(config: &ConPtyConfig) -> io::Result<PtyPair> {
        unsafe {
            let mut pty_input_read = HANDLE::default();
            let mut pty_input_write = HANDLE::default();
            let mut pty_output_read = HANDLE::default();
            let mut pty_output_write = HANDLE::default();

            CreatePipe(&mut pty_input_read, &mut pty_input_write, None, 65536)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            CreatePipe(&mut pty_output_read, &mut pty_output_write, None, 65536)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            let size = COORD {
                X: config.cols as i16,
                Y: config.rows as i16,
            };
            let hpc = CreatePseudoConsole(size, pty_input_read, pty_output_write, 0)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            let _ = CloseHandle(pty_input_read);
            let _ = CloseHandle(pty_output_write);

            let mut attr_size: usize = 0;
            let _ = InitializeProcThreadAttributeList(
                Some(LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut())),
                1,
                Some(0),
                &mut attr_size,
            );

            let mut attr_buf = vec![0u8; attr_size];
            let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut _);

            InitializeProcThreadAttributeList(Some(attr_list), 1, Some(0), &mut attr_size)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(hpc.0 as *const _),
                std::mem::size_of::<HPCON>(),
                None,
                None,
            )
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            // Build environment block
            let env_block: Option<Vec<u16>> = if !config.env.is_empty() {
                let mut env_map: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for (key, val) in std::env::vars() {
                    env_map.insert(key, val);
                }
                for (key, val) in &config.env {
                    env_map.insert(key.clone(), val.clone());
                }
                let mut block: Vec<u16> = Vec::new();
                for (key, val) in &env_map {
                    let entry = format!("{}={}", key, val);
                    block.extend(entry.encode_utf16());
                    block.push(0);
                }
                block.push(0);
                Some(block)
            } else {
                None
            };

            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = attr_list;

            let mut pi = PROCESS_INFORMATION::default();
            let (exe, args) = split_shell_command(&config.shell);
            let cmdline = if args.is_empty() {
                quote_cmd(&exe)
            } else {
                format!("{} {}", quote_cmd(&exe), args)
            };
            let mut cmd: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
            let app_name_buf: Option<Vec<u16>> =
                if exe.contains('\\') || exe.contains('/') || exe.contains(':') {
                    Some(exe.encode_utf16().chain(std::iter::once(0)).collect())
                } else {
                    None
                };
            let app_name = app_name_buf
                .as_ref()
                .map(|b| PCWSTR(b.as_ptr()))
                .unwrap_or(PCWSTR::null());

            let env_ptr = env_block
                .as_ref()
                .map(|b| b.as_ptr() as *const std::ffi::c_void);
            let create_flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT;

            CreateProcessW(
                app_name,
                Some(PWSTR(cmd.as_mut_ptr())),
                None,
                None,
                false,
                create_flags,
                env_ptr,
                None,
                &si.StartupInfo,
                &mut pi,
            )
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            DeleteProcThreadAttributeList(attr_list);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);

            let child_pid = pi.dwProcessId;
            let read_file = File::from_raw_handle(pty_output_read.0 as RawHandle);
            let write_file = File::from_raw_handle(pty_input_write.0 as RawHandle);

            Ok(PtyPair {
                master_read: Arc::new(Mutex::new(Box::new(read_file))),
                master_write: Arc::new(Mutex::new(Box::new(write_file))),
                child_pid,
                pseudo_console: Some(PseudoConsoleHandle(hpc)),
            })
        }
    }
}

#[cfg(windows)]
fn windows_resize(pc: &PseudoConsoleHandle, cols: u16, rows: u16) -> io::Result<()> {
    use windows::Win32::System::Console::{COORD, ResizePseudoConsole};
    let size = COORD {
        X: cols as i16,
        Y: rows as i16,
    };
    unsafe {
        ResizePseudoConsole(pc.0, size)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
    }
}

/// Spawn a PTY with the given configuration
pub fn spawn_pty(config: ConPtyConfig) -> io::Result<PtyPair> {
    #[cfg(windows)]
    {
        windows_impl::spawn(&config)
    }
    #[cfg(not(windows))]
    {
        let _ = config;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Non-Windows PTY not yet implemented",
        ))
    }
}
