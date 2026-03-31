//! IPC -> GPUI channel bridge
//!
//! The IPC handler thread cannot access GPUI `cx` directly.
//! This module bridges IPC requests to the GPUI event loop via flume channels.

use serde::{Deserialize, Serialize};

/// Command sent from IPC thread to GPUI event loop
#[derive(Debug)]
#[allow(dead_code)]
pub enum GpuiCommand {
    SplitWindow {
        horizontal: bool,
        working_directory: Option<String>,
        command: Option<String>,
        env: Vec<(String, String)>,
        resp_tx: flume::Sender<GpuiResponse>,
    },
    SendKeys {
        pane_id: u32,
        data: Vec<u8>,
        resp_tx: flume::Sender<GpuiResponse>,
    },
    SendKeysAll {
        data: Vec<u8>,
        resp_tx: flume::Sender<GpuiResponse>,
    },
    ListPanes {
        resp_tx: flume::Sender<GpuiResponse>,
    },
    SelectPane {
        pane_id: u32,
        resp_tx: flume::Sender<GpuiResponse>,
    },
    KillPane {
        pane_id: u32,
        resp_tx: flume::Sender<GpuiResponse>,
    },
    KillAllPanes {
        resp_tx: flume::Sender<GpuiResponse>,
    },
    CapturePane {
        pane_id: u32,
        resp_tx: flume::Sender<GpuiResponse>,
    },
}

/// Response from GPUI event loop back to IPC thread
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GpuiResponse {
    SplitOk { pane_id: u32 },
    SendKeysOk,
    SendKeysAllOk { count: usize },
    PaneList(Vec<PaneInfo>),
    SelectPaneOk,
    KillPaneOk,
    KillAllPanesOk { count: usize },
    PaneContent(String),
    Error(String),
}

/// Information about a single pane (tmux-compatible fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane_id: u32,
    pub width: u16,
    pub height: u16,
    pub active: bool,
}

/// Type alias for the command channel sender
pub type CommandSender = flume::Sender<GpuiCommand>;
/// Type alias for the command channel receiver
pub type CommandReceiver = flume::Receiver<GpuiCommand>;

/// Create a bounded command channel for IPC->GPUI bridge
pub fn create_channel() -> (CommandSender, CommandReceiver) {
    flume::bounded(32)
}

/// Parse `-t %N` or `-t%N` target pane ID from args, returning (pane_id, remaining_args)
/// Parse `-t %N` target from args. Returns (pane_id, remaining_args, is_broadcast).
/// `-t *`, `-t all`, or `-a` flag sets is_broadcast=true.
fn parse_target_pane(args: &[String]) -> (u32, Vec<String>, bool) {
    let mut pane_id: u32 = 0;
    let mut remaining = Vec::new();
    let mut skip_next = false;
    let mut broadcast = false;

    for (i, arg) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "-a" {
            broadcast = true;
        } else if arg == "-t" {
            if let Some(target) = args.get(i + 1) {
                let trimmed = target.trim_start_matches('%');
                if trimmed == "*" || trimmed == "all" {
                    broadcast = true;
                } else {
                    pane_id = trimmed.parse().unwrap_or(0);
                }
                skip_next = true;
            }
        } else if arg.starts_with("-t") {
            let trimmed = arg[2..].trim_start_matches('%');
            if trimmed == "*" || trimmed == "all" {
                broadcast = true;
            } else {
                pane_id = trimmed.parse().unwrap_or(0);
            }
        } else {
            remaining.push(arg.clone());
        }
    }

    (pane_id, remaining, broadcast)
}

/// Register IPC command handlers that forward to the GPUI bridge channel
pub fn register_handlers(server: &super::IpcServer, cmd_tx: CommandSender) {
    // ── split-window ────────────────────────────────────────────────
    let tx = cmd_tx.clone();
    server.on_command("split-window", move |req| {
        let horizontal = req.args.iter().any(|a| a == "-h");
        let mut working_directory = None;
        let mut command_parts = Vec::new();
        let mut env_vars: Vec<(String, String)> = Vec::new();
        let mut after_separator = false;
        let mut i = 0usize;

        while i < req.args.len() {
            let arg = &req.args[i];
            if after_separator {
                command_parts.push(arg.clone());
                i += 1;
                continue;
            }
            match arg.as_str() {
                "--" => {
                    after_separator = true;
                    i += 1;
                }
                "-c" => {
                    if let Some(dir) = req.args.get(i + 1) {
                        working_directory = Some(dir.clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                s if s.starts_with("-c") && s.len() > 2 => {
                    working_directory = Some(s[2..].to_string());
                    i += 1;
                }
                // Parse -e KEY=VALUE (tmux env var flag for team_name etc.)
                "-e" => {
                    if let Some(kv) = req.args.get(i + 1) {
                        if let Some((k, v)) = kv.split_once('=') {
                            env_vars.push((k.to_string(), v.to_string()));
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                s if s.starts_with("-e") && s.len() > 2 => {
                    let kv = &s[2..];
                    if let Some((k, v)) = kv.split_once('=') {
                        env_vars.push((k.to_string(), v.to_string()));
                    }
                    i += 1;
                }
                "-h" | "-v" | "-d" | "-b" | "-f" | "-P" => {
                    i += 1;
                }
                "-l" | "-p" | "-t" | "-F" => {
                    i += 2; // skip flag + value
                }
                s if s.starts_with("-l")
                    || s.starts_with("-p")
                    || s.starts_with("-t")
                    || s.starts_with("-F") =>
                {
                    let _ = s;
                    i += 1;
                }
                _ => {
                    command_parts.push(arg.clone());
                    i += 1;
                }
            }
        }

        let command = if command_parts.is_empty() {
            None
        } else {
            Some(command_parts.join(" "))
        };

        let (resp_tx, resp_rx) = flume::bounded(1);
        let cmd = GpuiCommand::SplitWindow {
            horizontal,
            working_directory,
            command,
            env: env_vars,
            resp_tx,
        };

        if tx.send(cmd).is_err() {
            return super::IpcResponse::err(req.id, "GPUI bridge disconnected");
        }

        match resp_rx.recv_timeout(std::time::Duration::from_secs(15)) {
            Ok(GpuiResponse::SplitOk { pane_id }) => super::IpcResponse::ok(
                req.id,
                serde_json::json!({ "pane_id": format!("%{}", pane_id) }),
            ),
            Ok(GpuiResponse::Error(e)) => super::IpcResponse::err(req.id, &e),
            Ok(_) => super::IpcResponse::err(req.id, "unexpected response"),
            Err(_) => super::IpcResponse::err(req.id, "GPUI response timeout"),
        }
    });

    // ── send-keys ───────────────────────────────────────────────────
    let tx = cmd_tx.clone();
    server.on_command("send-keys", move |req| {
        let (pane_id, key_args, broadcast) = parse_target_pane(&req.args);
        let data = convert_tmux_keys(&key_args);

        // Broadcast mode: -t * sends to all panes
        if broadcast {
            log::info!("[IPC] send-keys BROADCAST: data_len={}", data.len());
            let (resp_tx, resp_rx) = flume::bounded(1);
            let cmd = GpuiCommand::SendKeysAll { data, resp_tx };

            if tx.send(cmd).is_err() {
                return super::IpcResponse::err(req.id, "GPUI bridge disconnected");
            }

            return match resp_rx.recv_timeout(std::time::Duration::from_secs(15)) {
                Ok(GpuiResponse::SendKeysAllOk { count }) => {
                    super::IpcResponse::ok(req.id, serde_json::json!({ "count": count }))
                }
                Ok(GpuiResponse::Error(e)) => super::IpcResponse::err(req.id, &e),
                Ok(_) => super::IpcResponse::err(req.id, "unexpected response"),
                Err(_) => super::IpcResponse::err(req.id, "GPUI response timeout"),
            };
        }

        let (resp_tx, resp_rx) = flume::bounded(1);
        let cmd = GpuiCommand::SendKeys {
            pane_id,
            data,
            resp_tx,
        };

        if tx.send(cmd).is_err() {
            return super::IpcResponse::err(req.id, "GPUI bridge disconnected");
        }

        match resp_rx.recv_timeout(std::time::Duration::from_secs(15)) {
            Ok(GpuiResponse::SendKeysOk) => {
                super::IpcResponse::ok(req.id, serde_json::json!({}))
            }
            Ok(GpuiResponse::Error(e)) => super::IpcResponse::err(req.id, &e),
            Ok(_) => super::IpcResponse::err(req.id, "unexpected response"),
            Err(_) => super::IpcResponse::err(req.id, "GPUI response timeout"),
        }
    });

    // ── list-panes ──────────────────────────────────────────────────
    let tx = cmd_tx.clone();
    server.on_command("list-panes", move |req| {
        let (resp_tx, resp_rx) = flume::bounded(1);
        let cmd = GpuiCommand::ListPanes { resp_tx };

        if tx.send(cmd).is_err() {
            return super::IpcResponse::err(req.id, "GPUI bridge disconnected");
        }

        match resp_rx.recv_timeout(std::time::Duration::from_secs(15)) {
            Ok(GpuiResponse::PaneList(panes)) => {
                super::IpcResponse::ok(req.id, serde_json::json!({ "panes": panes }))
            }
            Ok(GpuiResponse::Error(e)) => super::IpcResponse::err(req.id, &e),
            Ok(_) => super::IpcResponse::err(req.id, "unexpected response"),
            Err(_) => super::IpcResponse::err(req.id, "GPUI response timeout"),
        }
    });

    // ── select-pane ─────────────────────────────────────────────────
    let tx = cmd_tx.clone();
    server.on_command("select-pane", move |req| {
        let (pane_id, _remaining, _broadcast) = parse_target_pane(&req.args);

        let (resp_tx, resp_rx) = flume::bounded(1);
        let cmd = GpuiCommand::SelectPane { pane_id, resp_tx };

        if tx.send(cmd).is_err() {
            return super::IpcResponse::err(req.id, "GPUI bridge disconnected");
        }

        match resp_rx.recv_timeout(std::time::Duration::from_secs(15)) {
            Ok(GpuiResponse::SelectPaneOk) => {
                super::IpcResponse::ok(req.id, serde_json::json!({}))
            }
            Ok(GpuiResponse::Error(e)) => super::IpcResponse::err(req.id, &e),
            Ok(_) => super::IpcResponse::err(req.id, "unexpected response"),
            Err(_) => super::IpcResponse::err(req.id, "GPUI response timeout"),
        }
    });

    // ── capture-pane ────────────────────────────────────────────────
    let tx = cmd_tx.clone();
    server.on_command("capture-pane", move |req| {
        let (pane_id, _remaining, _broadcast) = parse_target_pane(&req.args);

        let (resp_tx, resp_rx) = flume::bounded(1);
        let cmd = GpuiCommand::CapturePane { pane_id, resp_tx };

        if tx.send(cmd).is_err() {
            return super::IpcResponse::err(req.id, "GPUI bridge disconnected");
        }

        match resp_rx.recv_timeout(std::time::Duration::from_secs(15)) {
            Ok(GpuiResponse::PaneContent(content)) => {
                super::IpcResponse::ok(req.id, serde_json::json!({ "content": content }))
            }
            Ok(GpuiResponse::Error(e)) => super::IpcResponse::err(req.id, &e),
            Ok(_) => super::IpcResponse::err(req.id, "unexpected response"),
            Err(_) => super::IpcResponse::err(req.id, "GPUI response timeout"),
        }
    });

    // ── kill-pane ───────────────────────────────────────────────────
    let tx = cmd_tx;
    server.on_command("kill-pane", move |req| {
        let (pane_id, _remaining, broadcast) = parse_target_pane(&req.args);

        // Kill all split panes: -a or -t *
        if broadcast {
            log::info!("[IPC] kill-pane -a: killing all split panes");
            let (resp_tx, resp_rx) = flume::bounded(1);
            let cmd = GpuiCommand::KillAllPanes { resp_tx };

            if tx.send(cmd).is_err() {
                return super::IpcResponse::err(req.id, "GPUI bridge disconnected");
            }

            return match resp_rx.recv_timeout(std::time::Duration::from_secs(15)) {
                Ok(GpuiResponse::KillAllPanesOk { count }) => {
                    log::info!("[IPC] kill-pane -a OK: killed {} panes", count);
                    super::IpcResponse::ok(req.id, serde_json::json!({ "killed": count }))
                }
                Ok(GpuiResponse::Error(e)) => super::IpcResponse::err(req.id, &e),
                Ok(_) => super::IpcResponse::err(req.id, "unexpected response"),
                Err(_) => super::IpcResponse::err(req.id, "GPUI response timeout"),
            };
        }

        let (resp_tx, resp_rx) = flume::bounded(1);
        let cmd = GpuiCommand::KillPane { pane_id, resp_tx };

        if tx.send(cmd).is_err() {
            return super::IpcResponse::err(req.id, "GPUI bridge disconnected");
        }

        match resp_rx.recv_timeout(std::time::Duration::from_secs(15)) {
            Ok(GpuiResponse::KillPaneOk) => {
                super::IpcResponse::ok(req.id, serde_json::json!({}))
            }
            Ok(GpuiResponse::Error(e)) => super::IpcResponse::err(req.id, &e),
            Ok(_) => super::IpcResponse::err(req.id, "unexpected response"),
            Err(_) => super::IpcResponse::err(req.id, "GPUI response timeout"),
        }
    });
}

/// Convert tmux key names to raw bytes for PTY input.
///
/// Handles standard tmux key names: Enter, Escape, Space, Tab, BSpace,
/// C-c, C-d, C-z, arrow keys, and arbitrary C-X control sequences.
/// On Windows, also detects bare absolute paths sent with Enter and
/// wraps them in `cd /d "..."` for cmd.exe compatibility.
pub fn convert_tmux_keys(args: &[String]) -> Vec<u8> {
    #[cfg(windows)]
    {
        // tmux-compatible clients occasionally send only a quoted absolute
        // path + Enter to move directories. cmd.exe treats that as a command
        // name and fails. Normalize it to `cd /d "<path>"` for compatibility.
        let newline_key = args
            .last()
            .map(|k| {
                matches!(
                    k.as_str(),
                    "Enter" | "enter" | "Return" | "KPEnter" | "NEnter" | "C-m" | "C-M"
                )
            })
            .unwrap_or(false);
        if args.len() >= 2 && newline_key {
            let raw = args[..args.len() - 1].join("");
            let trimmed = raw.trim();
            let unquoted = trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .or_else(|| trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
                .unwrap_or(trimmed)
                .trim();

            let bytes = unquoted.as_bytes();
            let looks_abs_win_path =
                bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/');

            if looks_abs_win_path {
                return format!("cd /d \"{}\"\r", unquoted).into_bytes();
            }
        }
    }

    let mut data = Vec::new();
    for arg in args {
        match arg.as_str() {
            "Enter" => data.push(b'\r'),
            "Escape" => data.push(0x1b),
            "Space" => data.push(b' '),
            "Tab" => data.push(b'\t'),
            "BSpace" => data.push(0x7f),
            "C-c" => data.push(0x03),
            "C-d" => data.push(0x04),
            "C-z" => data.push(0x1a),
            "C-l" => data.push(0x0c),
            "C-a" => data.push(0x01),
            "C-e" => data.push(0x05),
            "C-k" => data.push(0x0b),
            "C-u" => data.push(0x15),
            "C-w" => data.push(0x17),
            "C-\\" => data.push(0x1c),
            "Up" => data.extend_from_slice(b"\x1b[A"),
            "Down" => data.extend_from_slice(b"\x1b[B"),
            "Right" => data.extend_from_slice(b"\x1b[C"),
            "Left" => data.extend_from_slice(b"\x1b[D"),
            other => {
                // Check for C-X pattern (single char control)
                if other.starts_with("C-") && other.len() == 3 {
                    let ch = other.as_bytes()[2].to_ascii_lowercase();
                    let ctrl = ch.wrapping_sub(b'a').wrapping_add(1);
                    data.push(ctrl);
                } else {
                    // Literal string
                    data.extend_from_slice(other.as_bytes());
                }
            }
        }
    }
    data
}
