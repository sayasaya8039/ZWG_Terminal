//! Shell detection and launcher for Windows
//! Supports: PowerShell, PowerShell 7, CMD, WSL, Git Bash

use std::path::Path;

/// Supported shell types
#[derive(Debug, Clone, PartialEq)]
pub enum ShellType {
    PowerShell,
    Pwsh,
    Cmd,
    Wsl,
    GitBash,
}

/// Detect the default shell on the system
pub fn detect_default_shell() -> String {
    if cfg!(windows) {
        // Prefer PowerShell 7 (pwsh) if available
        if which_exists("pwsh.exe") {
            return "pwsh.exe".to_string();
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
        // PowerShell (always available on Windows)
        shells.push((ShellType::PowerShell, "powershell.exe".to_string()));

        // PowerShell 7
        if which_exists("pwsh.exe") {
            shells.push((ShellType::Pwsh, "pwsh.exe".to_string()));
        }

        // CMD
        shells.push((ShellType::Cmd, "cmd.exe".to_string()));

        // WSL
        if which_exists("wsl.exe") {
            shells.push((ShellType::Wsl, "wsl.exe".to_string()));
        }

        // Git Bash
        let git_bash_paths = [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ];
        for path in &git_bash_paths {
            if Path::new(path).exists() {
                shells.push((ShellType::GitBash, format!("\"{}\" --login", path)));
                break;
            }
        }
    }

    shells
}

fn which_exists(cmd: &str) -> bool {
    if cfg!(windows) {
        std::process::Command::new("where")
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
