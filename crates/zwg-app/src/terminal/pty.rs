//! Cross-platform PTY implementation
//! Windows: ConPTY (CreatePseudoConsole)

use parking_lot::Mutex;
use std::io::{self, Read, Write};
use std::sync::Arc;

#[derive(Clone)]
pub struct ConPtyConfig {
    pub shell: String,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

impl Default for ConPtyConfig {
    fn default() -> Self {
        Self {
            shell: String::new(),
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
    #[cfg(windows)]
    #[allow(dead_code)] // kept for Drop cleanup only
    process_handle: Option<ProcessHandle>,
}

unsafe impl Send for PtyPair {}
// M9: Safety: all fields are behind Arc<Mutex<_>> or are POD types.
// Windows HANDLE types lack Send/Sync, but our fields are wrapped in Mutex locks.
unsafe impl Sync for PtyPair {}

impl PtyPair {
    pub fn write_input(&self, data: &[u8]) -> io::Result<usize> {
        let mut writer = self.master_write.lock();
        writer.write_all(data)?;
        Ok(data.len())
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
impl Drop for PseudoConsoleHandle {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Console::ClosePseudoConsole(self.0);
        }
        log::debug!("ConPTY pseudo console closed");
    }
}

/// RAII wrapper for Windows process handle
#[cfg(windows)]
struct ProcessHandle(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for ProcessHandle {}
#[cfg(windows)]
unsafe impl Sync for ProcessHandle {}

#[cfg(windows)]
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

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
        CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
        STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
    };
    use windows::core::{PCWSTR, PWSTR};

    /// H7: RAII wrapper for pipe HANDLEs to prevent leaks on error paths
    struct PipeHandle(HANDLE);
    impl Drop for PipeHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }
    impl PipeHandle {
        fn take(mut self) -> HANDLE {
            let h = self.0;
            self.0 = HANDLE::default();
            std::mem::forget(self);
            h
        }
    }

    /// Validate that a shell path doesn't contain obvious injection characters
    fn validate_shell_path(path: &str) -> io::Result<()> {
        let forbidden = ['|', '&', ';', '`', '$', '(', ')', '{', '}', '<', '>'];
        if path.chars().any(|c| forbidden.contains(&c)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Shell path contains forbidden character: {}", path),
            ));
        }
        Ok(())
    }

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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn split_empty_returns_powershell() {
            let (exe, args) = split_shell_command("");
            assert_eq!(exe, "powershell.exe");
            assert!(args.is_empty());
        }

        #[test]
        fn split_whitespace_only_returns_powershell() {
            let (exe, args) = split_shell_command("   ");
            assert_eq!(exe, "powershell.exe");
            assert!(args.is_empty());
        }

        #[test]
        fn split_simple_exe() {
            let (exe, args) = split_shell_command("cmd.exe");
            assert_eq!(exe, "cmd.exe");
            assert!(args.is_empty());
        }

        #[test]
        fn split_exe_with_args() {
            let (exe, args) = split_shell_command("cmd.exe /K echo hello");
            assert_eq!(exe, "cmd.exe");
            assert_eq!(args, "/K echo hello");
        }

        #[test]
        fn split_quoted_path() {
            let (exe, args) = split_shell_command(r#""C:\Program Files\shell.exe" --flag"#);
            assert_eq!(exe, r"C:\Program Files\shell.exe");
            assert_eq!(args, "--flag");
        }

        #[test]
        fn split_quoted_no_args() {
            let (exe, args) = split_shell_command(r#""C:\shell.exe""#);
            assert_eq!(exe, r"C:\shell.exe");
            assert!(args.is_empty());
        }

        #[test]
        fn quote_cmd_no_spaces() {
            assert_eq!(quote_cmd("cmd.exe"), "cmd.exe");
        }

        #[test]
        fn quote_cmd_with_spaces() {
            let q = quote_cmd(r"C:\Program Files\shell.exe");
            assert!(q.starts_with('"'));
            assert!(q.ends_with('"'));
            assert!(q.contains(r"C:\Program Files\shell.exe"));
        }

        #[test]
        fn validate_clean_path_ok() {
            assert!(validate_shell_path("cmd.exe").is_ok());
            assert!(validate_shell_path(r"C:\Windows\System32\cmd.exe").is_ok());
            assert!(validate_shell_path("powershell.exe").is_ok());
        }

        #[test]
        fn validate_pipe_rejected() {
            assert!(validate_shell_path("cmd.exe | evil").is_err());
        }

        #[test]
        fn validate_ampersand_rejected() {
            assert!(validate_shell_path("cmd.exe & evil").is_err());
        }

        #[test]
        fn validate_semicolon_rejected() {
            assert!(validate_shell_path("cmd.exe; evil").is_err());
        }

        #[test]
        fn validate_backtick_rejected() {
            assert!(validate_shell_path("cmd.exe `evil`").is_err());
        }

        #[test]
        fn validate_dollar_rejected() {
            assert!(validate_shell_path("$(evil)").is_err());
        }

        #[test]
        fn validate_angle_brackets_rejected() {
            assert!(validate_shell_path("cmd.exe > output").is_err());
            assert!(validate_shell_path("cmd.exe < input").is_err());
        }

        #[test]
        fn validate_braces_rejected() {
            assert!(validate_shell_path("cmd.exe {evil}").is_err());
        }

        #[test]
        fn validate_parens_rejected() {
            assert!(validate_shell_path("cmd.exe (evil)").is_err());
        }
    }

    pub fn spawn(config: &ConPtyConfig) -> io::Result<PtyPair> {
        // Validate shell path before proceeding
        validate_shell_path(&config.shell)?;

        unsafe {
            let mut pty_input_read = HANDLE::default();
            let mut pty_input_write = HANDLE::default();
            let mut pty_output_read = HANDLE::default();
            let mut pty_output_write = HANDLE::default();

            CreatePipe(&mut pty_input_read, &mut pty_input_write, None, 131_072)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            // H7: wrap in RAII immediately to prevent leak if second CreatePipe fails
            let pty_input_read = PipeHandle(pty_input_read);
            let pty_input_write = PipeHandle(pty_input_write);

            CreatePipe(&mut pty_output_read, &mut pty_output_write, None, 131_072)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            let pty_output_read = PipeHandle(pty_output_read);
            let pty_output_write = PipeHandle(pty_output_write);

            // M7: clamp to i16::MAX to prevent overflow on u16→i16 cast
            let safe_cols = config.cols.min(i16::MAX as u16) as i16;
            let safe_rows = config.rows.min(i16::MAX as u16) as i16;
            let size = COORD {
                X: safe_cols.max(1),
                Y: safe_rows.max(1),
            };
            // H7: PipeHandle RAII will auto-close on error path
            let hpc = CreatePseudoConsole(size, pty_input_read.0, pty_output_write.0, 0)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            // RAII guard: ClosePseudoConsole on error via Drop
            let hpc_raw = hpc.0;
            let mut pc_guard = Some(PseudoConsoleHandle(hpc));

            // These handles are now owned by the pseudo console — drop our copies
            drop(pty_input_read);
            drop(pty_output_write);

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
                Some(hpc_raw as *const _),
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
            // Fix: Prevent parent's redirected stdout/stderr from being duplicated
            // to the child. Without this flag, when the parent process runs inside
            // another terminal (e.g., Claude Code, VS Code), Windows duplicates the
            // parent's non-console handles to the child, bypassing ConPTY entirely.
            // See: https://github.com/microsoft/terminal/issues/11276
            si.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;

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
            // pi.hProcess kept alive via ProcessHandle for child process monitoring

            let child_pid = pi.dwProcessId;
            // H7: take ownership from PipeHandle RAII wrappers
            let read_file = File::from_raw_handle(pty_output_read.take().0 as RawHandle);
            let write_file = File::from_raw_handle(pty_input_write.take().0 as RawHandle);

            Ok(PtyPair {
                master_read: Arc::new(Mutex::new(Box::new(read_file))),
                master_write: Arc::new(Mutex::new(Box::new(write_file))),
                child_pid,
                pseudo_console: pc_guard.take(),
                process_handle: Some(ProcessHandle(pi.hProcess)),
            })
        }
    }
}

#[cfg(windows)]
fn windows_resize(pc: &PseudoConsoleHandle, cols: u16, rows: u16) -> io::Result<()> {
    use windows::Win32::System::Console::{COORD, ResizePseudoConsole};
    // M7: clamp to prevent i16 overflow
    let size = COORD {
        X: cols.min(i16::MAX as u16) as i16,
        Y: rows.min(i16::MAX as u16) as i16,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(windows)]
    #[test]
    fn spawn_pty_cmd_echo_produces_output() {
        let pty = spawn_pty(ConPtyConfig {
            shell: "cmd.exe /c echo ZWG_PTY_SMOKE_TEST".into(),
            cols: 80,
            rows: 24,
            env: Vec::new(),
        })
        .expect("spawn_pty should succeed for cmd.exe smoke test");

        let reader = pty.reader();
        let mut output = Vec::new();
        let mut buf = [0u8; 512];

        loop {
            let read = {
                let mut guard = reader.lock();
                guard.read(&mut buf)
            }
            .expect("reading PTY output should succeed");

            if read == 0 {
                break;
            }

            output.extend_from_slice(&buf[..read]);

            if output
                .windows("ZWG_PTY_SMOKE_TEST".len())
                .any(|window| window == b"ZWG_PTY_SMOKE_TEST")
            {
                break;
            }
        }

        let rendered = String::from_utf8_lossy(&output);
        assert!(
            rendered.contains("ZWG_PTY_SMOKE_TEST"),
            "expected cmd.exe output, got {:?}",
            rendered
        );
    }

    #[cfg(windows)]
    #[test]
    fn spawn_pty_powershell_read_host_accepts_unicode_input() {
        let script_path = std::env::temp_dir().join(format!(
            "zwg_unicode_input_{}_{}.ps1",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::write(
            &script_path,
            concat!(
                "$line = [Console]::In.ReadLine()\n",
                "[Console]::WriteLine('ZWG_UNICODE_TEST:' + ",
                "[BitConverter]::ToString([Text.Encoding]::UTF8.GetBytes($line)))\n",
            ),
        )
        .expect("writing unicode input powershell script should succeed");

        let pty = spawn_pty(ConPtyConfig {
            shell: format!(
                "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
                script_path.display()
            ),
            cols: 120,
            rows: 32,
            env: Vec::new(),
        })
        .expect("spawn_pty should succeed for powershell unicode input test");

        let unicode_line = "・→↓\r";
        pty.write_input(unicode_line.as_bytes())
            .expect("writing unicode input line should succeed");

        let reader = pty.reader();
        let mut output = Vec::new();
        let mut buf = [0u8; 512];
        let expected = "ZWG_UNICODE_TEST:E3-83-BB-E2-86-92-E2-86-93";

        loop {
            let read = {
                let mut guard = reader.lock();
                guard.read(&mut buf)
            }
            .expect("reading PTY output should succeed");

            if read == 0 {
                break;
            }

            output.extend_from_slice(&buf[..read]);

            if output
                .windows(expected.len())
                .any(|window| window == expected.as_bytes())
            {
                break;
            }
        }

        let rendered = String::from_utf8_lossy(&output);
        assert!(
            rendered.contains(expected),
            "expected powershell to echo unicode input as utf8 hex, got {:?}",
            rendered
        );

        let _ = std::fs::remove_file(script_path);
    }

    #[cfg(windows)]
    #[test]
    fn spawn_pty_powershell_read_key_ignores_raw_utf8_unicode_input() {
        let script_path = std::env::temp_dir().join(format!(
            "zwg_unicode_readkey_{}_{}.ps1",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::write(
            &script_path,
            concat!(
                "$deadline = [DateTime]::UtcNow.AddSeconds(2)\n",
                "$events = New-Object System.Collections.Generic.List[string]\n",
                "while ([DateTime]::UtcNow -lt $deadline -and $events.Count -lt 4) {\n",
                "  if ([Console]::KeyAvailable) {\n",
                "    $key = [Console]::ReadKey($true)\n",
                "    $events.Add(('char={0};key={1};vk={2}' -f [int][char]$key.KeyChar, $key.Key, $key.VirtualKeyCode))\n",
                "  } else {\n",
                "    Start-Sleep -Milliseconds 20\n",
                "  }\n",
                "}\n",
                "[Console]::WriteLine('ZWG_READKEY_TEST:' + ($events -join '|'))\n",
            ),
        )
        .expect("writing readkey powershell script should succeed");

        let pty = spawn_pty(ConPtyConfig {
            shell: format!(
                "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
                script_path.display()
            ),
            cols: 120,
            rows: 32,
            env: Vec::new(),
        })
        .expect("spawn_pty should succeed for powershell readkey test");

        let unicode_line = "・↓\r";
        pty.write_input(unicode_line.as_bytes())
            .expect("writing unicode readkey payload should succeed");

        let reader = pty.reader();
        let mut output = Vec::new();
        let mut buf = [0u8; 512];
        let expected_prefix = "ZWG_READKEY_TEST:";

        loop {
            let read = {
                let mut guard = reader.lock();
                guard.read(&mut buf)
            }
            .expect("reading PTY output should succeed");

            if read == 0 {
                break;
            }

            output.extend_from_slice(&buf[..read]);

            if output
                .windows(expected_prefix.len())
                .any(|window| window == expected_prefix.as_bytes())
            {
                break;
            }
        }

        let rendered = String::from_utf8_lossy(&output);
        assert!(
            rendered.contains(expected_prefix),
            "expected powershell to print readkey marker, got {:?}",
            rendered
        );
        assert!(
            !rendered.contains("char=12539"),
            "raw utf8 unexpectedly produced middle dot key event: {:?}",
            rendered
        );
        assert!(
            !rendered.contains("char=8595"),
            "raw utf8 unexpectedly produced down arrow unicode key event: {:?}",
            rendered
        );
        assert!(
            rendered.contains("char=13;key=Enter"),
            "expected raw readkey path to at least receive Enter, got {:?}",
            rendered
        );

        let _ = std::fs::remove_file(script_path);
    }

    #[cfg(windows)]
    #[test]
    fn spawn_pty_powershell_read_key_accepts_win32_unicode_input_records() {
        let script_path = std::env::temp_dir().join(format!(
            "zwg_unicode_readkey_win32_{}_{}.ps1",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        std::fs::write(
            &script_path,
            concat!(
                "$deadline = [DateTime]::UtcNow.AddSeconds(2)\n",
                "$events = New-Object System.Collections.Generic.List[string]\n",
                "while ([DateTime]::UtcNow -lt $deadline -and $events.Count -lt 4) {\n",
                "  if ([Console]::KeyAvailable) {\n",
                "    $key = [Console]::ReadKey($true)\n",
                "    $events.Add(('char={0};key={1};vk={2}' -f [int][char]$key.KeyChar, $key.Key, $key.VirtualKeyCode))\n",
                "  } else {\n",
                "    Start-Sleep -Milliseconds 20\n",
                "  }\n",
                "}\n",
                "[Console]::WriteLine('ZWG_READKEY_TEST:' + ($events -join '|'))\n",
            ),
        )
        .expect("writing win32 readkey powershell script should succeed");

        let pty = spawn_pty(ConPtyConfig {
            shell: format!(
                "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
                script_path.display()
            ),
            cols: 120,
            rows: 32,
            env: Vec::new(),
        })
        .expect("spawn_pty should succeed for powershell win32 readkey test");

        let payload = crate::terminal::win32_input::encode_win32_input_text("・↓");
        pty.write_input(&payload)
            .expect("writing win32 unicode input payload should succeed");

        let reader = pty.reader();
        let mut output = Vec::new();
        let mut buf = [0u8; 512];
        let expected_prefix = "ZWG_READKEY_TEST:";

        loop {
            let read = {
                let mut guard = reader.lock();
                guard.read(&mut buf)
            }
            .expect("reading PTY output should succeed");

            if read == 0 {
                break;
            }

            output.extend_from_slice(&buf[..read]);

            if output
                .windows(expected_prefix.len())
                .any(|window| window == expected_prefix.as_bytes())
            {
                break;
            }
        }

        let rendered = String::from_utf8_lossy(&output);
        assert!(
            rendered.contains(expected_prefix),
            "expected powershell to print readkey marker, got {:?}",
            rendered
        );
        assert!(
            rendered.contains("char=12539"),
            "expected middle dot key event from win32 input records, got {:?}",
            rendered
        );
        assert!(
            rendered.contains("char=8595"),
            "expected down arrow unicode key event from win32 input records, got {:?}",
            rendered
        );

        let _ = std::fs::remove_file(script_path);
    }

    struct PartialWriter {
        writes: Arc<Mutex<Vec<u8>>>,
        chunk_size: usize,
        calls: Arc<AtomicUsize>,
    }

    impl Write for PartialWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let take = buf.len().min(self.chunk_size);
            self.writes.lock().extend_from_slice(&buf[..take]);
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(take)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_input_retries_until_all_bytes_are_written() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let pair = PtyPair {
            master_read: Arc::new(Mutex::new(Box::new(Cursor::new(Vec::<u8>::new())))),
            master_write: Arc::new(Mutex::new(Box::new(PartialWriter {
                writes: writes.clone(),
                chunk_size: 3,
                calls: calls.clone(),
            }))),
            child_pid: 1,
            #[cfg(windows)]
            pseudo_console: None,
            #[cfg(windows)]
            process_handle: None,
        };

        let payload = b"paste payload";
        let written = pair
            .write_input(payload)
            .expect("write_input should succeed");

        assert_eq!(written, payload.len());
        assert_eq!(&*writes.lock(), payload);
        assert!(calls.load(Ordering::Relaxed) > 1);
    }
}
