//! Shell detection and launcher for Windows
//! Supports: PowerShell, PowerShell 7, CMD, WSL, Git Bash

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
#[cfg(windows)]
use std::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Supported shell types
#[derive(Debug, Clone, PartialEq)]
pub enum ShellType {
    PowerShell,
    Pwsh,
    Cmd,
    Wsl,
    GitBash,
}

/// Known paths for shells on Windows (checked via Path::exists instead of spawning processes)
#[cfg(windows)]
const KNOWN_PWSH_PATHS: &[&str] = &[
    r"C:\Program Files\PowerShell\7\pwsh.exe",
    r"C:\Program Files (x86)\PowerShell\7\pwsh.exe",
];

#[cfg(windows)]
const KNOWN_WSL_PATHS: &[&str] = &[
    r"C:\Windows\System32\wsl.exe",
];

#[cfg(windows)]
const KNOWN_GIT_BASH_PATHS: &[&str] = &[
    r"C:\Program Files\Git\bin\bash.exe",
    r"C:\Program Files (x86)\Git\bin\bash.exe",
];

/// Check if a shell exists by probing known filesystem paths first,
/// falling back to `where` only if no known path matches.
#[cfg(windows)]
fn shell_exists_fast(cmd: &str, known_paths: &[&str]) -> bool {
    // Fast path: check known filesystem locations (no process spawn)
    for path in known_paths {
        if Path::new(path).exists() {
            return true;
        }
    }
    // Slow fallback: spawn `where` for non-standard install locations
    hidden_command("where")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detect the default shell on the system
pub fn detect_default_shell() -> String {
    if cfg!(windows) {
        #[cfg(windows)]
        {
            // Prefer PowerShell 7 (pwsh) if available — use fast path check
            if shell_exists_fast("pwsh.exe", KNOWN_PWSH_PATHS) {
                return "pwsh.exe".to_string();
            }
        }
        "powershell.exe".to_string()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
    }
}

/// Detect all available shells on the system
#[allow(dead_code)]
pub fn detect_available_shells() -> Vec<(ShellType, String)> {
    let mut shells = Vec::new();

    if cfg!(windows) {
        #[cfg(windows)]
        {
            // PowerShell (always available on Windows)
            shells.push((ShellType::PowerShell, "powershell.exe".to_string()));

            // PowerShell 7 — fast path check
            if shell_exists_fast("pwsh.exe", KNOWN_PWSH_PATHS) {
                shells.push((ShellType::Pwsh, "pwsh.exe".to_string()));
            }

            // CMD (always available)
            shells.push((ShellType::Cmd, "cmd.exe".to_string()));

            // WSL — fast path check
            if shell_exists_fast("wsl.exe", KNOWN_WSL_PATHS) {
                shells.push((ShellType::Wsl, "wsl.exe".to_string()));
            }

            // Git Bash — direct path check (no fallback needed)
            for path in KNOWN_GIT_BASH_PATHS {
                if Path::new(path).exists() {
                    shells.push((ShellType::GitBash, format!("\"{}\" --login", path)));
                    break;
                }
            }
        }
    }

    shells
}

#[allow(dead_code)]
fn which_exists(cmd: &str) -> bool {
    if cfg!(windows) {
        hidden_command("where")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        std::process::Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(windows)]
fn hidden_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}
