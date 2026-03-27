//! CLI parser and IPC client for tmux-compatible subcommands.
//!
//! When ZWG is invoked with subcommands (e.g. `zwg split-window -h`), it operates
//! as an IPC client sending commands to the running GUI instance. This enables tmux
//! shim compatibility for Claude Code teammate-mode.
//!
//! **Design:** Unknown tmux subcommands MUST exit 0. Claude Code probes tmux with
//! various commands during detection; if any return non-zero it aborts teammate-mode.

use std::io::{BufRead, BufReader, Write};

#[derive(serde::Serialize)]
struct IpcRequest { id: u64, command: String, args: Vec<String> }

#[derive(serde::Deserialize)]
struct IpcResponse {
    success: bool,
    #[serde(default)] data: serde_json::Value,
    #[serde(default)] error: Option<String>,
}

/// Parsed tmux-compatible command from raw CLI arguments.
pub enum CliCommand {
    Gui,
    SplitWindow {
        horizontal: bool, start_directory: Option<String>,
        command: Vec<String>, print_info: bool, format: Option<String>,
    },
    SendKeys { target: Option<String>, keys: Vec<String> },
    ListPanes { format: Option<String> },
    SelectPane { target: String },
    DisplayMessage { print_stdout: bool, format: Option<String> },
    KillPane { target: String },
    CapturePane { target: Option<String>, print_stdout: bool },
    HasSession,
    NewSession { print_info: bool, format: Option<String> },
    NewWindow { print_info: bool, format: Option<String> },
    ListWindows { format: Option<String> },
    ShowOptions { option_name: Option<String> },
    SilentSuccess,
    Unknown(String),
    Version,
}

// ── Argument parsing ──────────────────────────────────────────────────────

pub fn parse_args() -> CliCommand {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() { return CliCommand::Gui; }
    if args.iter().any(|a| a == "--version" || a == "-V") { return CliCommand::Version; }

    let args = skip_global_flags(&args);
    if args.is_empty() { return CliCommand::Gui; }

    let subcmd = &args[0];
    let rest = &args[1..];
    match subcmd.as_str() {
        "split-window" => parse_split_window(rest),
        "send-keys" | "send-key" => parse_send_keys(rest),
        "list-panes" => parse_list_panes(rest),
        "select-pane" => parse_select_pane(rest),
        "display-message" | "display" => parse_display_message(rest),
        "kill-pane" => parse_kill_pane(rest),
        "capture-pane" => parse_capture_pane(rest),
        "has-session" => CliCommand::HasSession,
        "new-session" => parse_new_session(rest),
        "new-window" => parse_new_window(rest),
        "list-windows" => parse_list_windows(rest),
        "show-options" | "show-option" => parse_show_options(rest),
        "set-option" | "set" | "select-layout" | "resize-pane" | "break-pane" | "join-pane"
        | "kill-session" | "kill-window" | "move-pane" | "swap-pane" | "move-window"
        | "swap-window" | "rename-session" | "rename-window" | "respawn-pane"
        | "set-environment" | "set-window-option" | "setw" => CliCommand::SilentSuccess,
        _ if subcmd.starts_with('-') => CliCommand::Gui,
        other => CliCommand::Unknown(other.to_string()),
    }
}

fn skip_global_flags(args: &[String]) -> Vec<String> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-L" | "-S" | "-f" => { i += 2; }
            s if s.starts_with("-L") || s.starts_with("-S") || s.starts_with("-f") => { i += 1; }
            _ => break,
        }
    }
    args[i..].to_vec()
}

fn parse_split_window(args: &[String]) -> CliCommand {
    let (mut horiz, mut dir, mut cmd, mut pr, mut fmt) =
        (false, None, Vec::new(), false, None);
    let mut after_sep = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if after_sep { cmd.push(a.clone()); i += 1; continue; }
        match a.as_str() {
            "--" => { after_sep = true; i += 1; }
            "-h" => { horiz = true; i += 1; }
            "-v" => { horiz = false; i += 1; }
            "-P" => { pr = true; i += 1; }
            "-F" => { fmt = args.get(i+1).cloned(); i += 2; }
            s if s.starts_with("-F") && s.len() > 2 => { fmt = Some(s[2..].into()); i += 1; }
            "-c" => { dir = args.get(i+1).cloned(); i += 2; }
            s if s.starts_with("-c") && s.len() > 2 => { dir = Some(s[2..].into()); i += 1; }
            "-d" | "-b" => { i += 1; }
            "-l" | "-p" => { i += 2; }
            s if s.starts_with("-l") || s.starts_with("-p") => { i += 1; }
            "-t" => { i += 2; }
            s if s.starts_with("-t") => { i += 1; }
            _ => { cmd.push(a.clone()); i += 1; }
        }
    }
    CliCommand::SplitWindow {
        horizontal: horiz, start_directory: dir, command: cmd, print_info: pr, format: fmt,
    }
}

fn parse_send_keys(args: &[String]) -> CliCommand {
    let mut target = None;
    let mut keys = Vec::new();
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip { skip = false; continue; }
        if a == "-t" { target = args.get(i+1).cloned(); skip = true; }
        else if a.starts_with("-t") { target = Some(a[2..].into()); }
        else if a == "-l" { /* literal flag, ignore */ }
        else { keys.push(a.clone()); }
    }
    CliCommand::SendKeys { target, keys }
}

fn parse_list_panes(args: &[String]) -> CliCommand {
    let mut format = None;
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip { skip = false; continue; }
        if a == "-F" { format = args.get(i+1).cloned(); skip = true; }
        else if a.starts_with("-F") { format = Some(a[2..].into()); }
    }
    CliCommand::ListPanes { format }
}

fn parse_target_flag(args: &[String]) -> String {
    let mut target = String::new();
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip { skip = false; continue; }
        if a == "-t" { if let Some(t) = args.get(i+1) { target = t.clone(); skip = true; } }
        else if a.starts_with("-t") { target = a[2..].into(); }
    }
    target
}

fn parse_select_pane(args: &[String]) -> CliCommand {
    CliCommand::SelectPane { target: parse_target_flag(args) }
}

fn parse_kill_pane(args: &[String]) -> CliCommand {
    CliCommand::KillPane { target: parse_target_flag(args) }
}

fn parse_capture_pane(args: &[String]) -> CliCommand {
    let mut target = None;
    let mut print_stdout = false;
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip { skip = false; continue; }
        match a.as_str() {
            "-p" => print_stdout = true,
            "-e" | "-q" | "-J" | "-N" => {}
            "-t" => { target = args.get(i+1).cloned(); skip = true; }
            s if s.starts_with("-t") => { target = Some(s[2..].into()); }
            "-S" | "-E" | "-b" => { skip = true; }
            _ => {}
        }
    }
    CliCommand::CapturePane { target, print_stdout }
}

fn parse_display_message(args: &[String]) -> CliCommand {
    let mut print_stdout = false;
    let mut format = None;
    let mut skip = false;
    for a in args {
        if skip { skip = false; continue; }
        match a.as_str() {
            "-p" => print_stdout = true,
            "-t" => { skip = true; }
            s if s.starts_with("-t") || s.starts_with('-') => {}
            _ => { if format.is_none() { format = Some(a.clone()); } }
        }
    }
    CliCommand::DisplayMessage { print_stdout, format }
}

fn parse_new_session(args: &[String]) -> CliCommand {
    let mut print_info = false;
    let mut format = None;
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip { skip = false; continue; }
        match a.as_str() {
            "-P" => print_info = true,
            "-F" => { format = args.get(i+1).cloned(); skip = true; }
            "-d" | "-x" | "-y" | "-E" => {}
            "-s" | "-n" | "-t" => { skip = true; }
            s if s.starts_with("-F") => { format = Some(s[2..].into()); }
            _ => {}
        }
    }
    CliCommand::NewSession { print_info, format }
}

fn parse_new_window(args: &[String]) -> CliCommand {
    let mut print_info = false;
    let mut format = None;
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip { skip = false; continue; }
        match a.as_str() {
            "-P" => print_info = true,
            "-F" => { format = args.get(i+1).cloned(); skip = true; }
            "-d" | "-k" | "-a" | "-b" => {}
            "-t" | "-n" | "-e" => { skip = true; }
            s if s.starts_with("-F") => { format = Some(s[2..].into()); }
            _ => {}
        }
    }
    CliCommand::NewWindow { print_info, format }
}

fn parse_list_windows(args: &[String]) -> CliCommand {
    let mut format = None;
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip { skip = false; continue; }
        if a == "-F" { format = args.get(i+1).cloned(); skip = true; }
        else if a.starts_with("-F") { format = Some(a[2..].into()); }
        else if a == "-t" { skip = true; }
    }
    CliCommand::ListWindows { format }
}

fn parse_show_options(args: &[String]) -> CliCommand {
    let mut name = None;
    for a in args {
        match a.as_str() {
            "-g" | "-w" | "-s" | "-p" | "-q" | "-v" => {}
            s if s.starts_with('-') => {}
            _ => { if name.is_none() { name = Some(a.clone()); } }
        }
    }
    CliCommand::ShowOptions { option_name: name }
}

// ── run_client ────────────────────────────────────────────────────────────

pub fn run_client(command: CliCommand) -> ! {
    match command {
        CliCommand::Gui => std::process::exit(0),
        CliCommand::Version => { println!("tmux 3.4"); std::process::exit(0); }
        CliCommand::Unknown(_) | CliCommand::SilentSuccess => std::process::exit(0),
        CliCommand::HasSession => std::process::exit(0),
        CliCommand::ShowOptions { option_name } => run_show_options(option_name),
        CliCommand::DisplayMessage { print_stdout, format } => {
            run_display_message(print_stdout, format);
        }
        CliCommand::SplitWindow { horizontal, start_directory, command, print_info, format } => {
            run_split_window(horizontal, start_directory.as_deref(), &command, print_info, format);
        }
        CliCommand::SendKeys { target, keys } => {
            let mut a = Vec::new();
            if let Some(t) = target { a.push("-t".into()); a.push(t); }
            a.extend_from_slice(&keys);
            run_ipc_command("send-keys", a);
        }
        CliCommand::ListPanes { format } => {
            let mut a = Vec::new();
            if let Some(f) = format { a.push("-F".into()); a.push(f); }
            run_ipc_command_list_panes(a);
        }
        CliCommand::SelectPane { target } => {
            run_ipc_command("select-pane", vec!["-t".into(), target]);
        }
        CliCommand::KillPane { target } => {
            run_ipc_command("kill-pane", vec!["-t".into(), target]);
        }
        CliCommand::CapturePane { target, print_stdout } => {
            run_capture_pane(target, print_stdout);
        }
        CliCommand::NewSession { print_info, format }
        | CliCommand::NewWindow { print_info, format } => {
            run_new_session_or_window(print_info, format);
        }
        CliCommand::ListWindows { format } => run_list_windows(format),
    }
}

// ── Format expansion ──────────────────────────────────────────────────────

pub fn expand_tmux_format(
    fmt: &str, pane_id: u32, width: u16, height: u16, active: bool,
) -> String {
    let mut r = fmt.to_string();
    let pid = std::process::id().to_string();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "/".into());
    let shell = if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".into())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
    };
    let (ws, hs) = (width.to_string(), height.to_string());
    let act = if active { "1" } else { "0" };
    let tty = if cfg!(windows) { "conpty" } else { "/dev/pts/0" };

    // Pane
    r = r.replace("#{pane_id}", &format!("%{}", pane_id));
    r = r.replace("#{pane_index}", &pane_id.to_string());
    r = r.replace("#{pane_width}", &ws);
    r = r.replace("#{pane_height}", &hs);
    r = r.replace("#{pane_active}", act);
    r = r.replace("#{pane_pid}", &pid);
    r = r.replace("#{pane_tty}", tty);
    r = r.replace("#{pane_current_path}", &cwd);
    r = r.replace("#{pane_current_command}", &shell);
    r = r.replace("#{pane_title}", "zwg");
    r = r.replace("#{pane_start_command}", &shell);
    r = r.replace("#{pane_dead}", "0");
    r = r.replace("#{pane_in_mode}", "0");
    r = r.replace("#{pane_mode}", "");
    r = r.replace("#{pane_synchronized}", "0");
    // Cursor
    r = r.replace("#{cursor_x}", "0");
    r = r.replace("#{cursor_y}", "0");
    r = r.replace("#{cursor_character}", " ");
    // Window
    r = r.replace("#{window_id}", "@0");
    r = r.replace("#{window_index}", "0");
    r = r.replace("#{window_name}", "zwg");
    r = r.replace("#{window_active}", "1");
    r = r.replace("#{window_width}", &ws);
    r = r.replace("#{window_height}", &hs);
    r = r.replace("#{window_panes}", "1");
    // Session
    r = r.replace("#{session_name}", "zwg");
    r = r.replace("#{session_id}", "$0");
    r = r.replace("#{session_windows}", "1");
    r = r.replace("#{session_attached}", "1");
    r = r.replace("#{session_width}", &ws);
    r = r.replace("#{session_height}", &hs);
    // Client
    r = r.replace("#{client_tty}", tty);
    r = r.replace("#{client_width}", &ws);
    r = r.replace("#{client_height}", &hs);

    // Conditionals: #{?var,true,false}
    loop {
        let start = match r.find("#{?") { Some(s) => s, None => break };
        let end = match r[start..].find('}') { Some(e) => e, None => break };
        let inner = r[start + 3..start + end].to_string();
        let parts: Vec<&str> = inner.splitn(3, ',').collect();
        let repl = if parts.len() >= 2 { parts[1].to_string() } else { String::new() };
        r.replace_range(start..start + end + 1, &repl);
    }
    r
}

fn fmt_default(fmt: &str, pane_id: u32) -> String {
    expand_tmux_format(fmt, pane_id, 120, 40, true)
}

// ── IPC run helpers ───────────────────────────────────────────────────────

fn run_display_message(print_stdout: bool, format: Option<String>) -> ! {
    let fmt = format.as_deref().unwrap_or("#{session_name}");
    let pane_id = send_ipc_request(&IpcRequest {
        id: 1, command: "list-panes".into(), args: vec![],
    }).ok()
        .filter(|r| r.success)
        .and_then(|r| r.data.get("panes")?.as_array()?.first()?.get("pane_id")?.as_u64())
        .unwrap_or(0) as u32;
    if print_stdout { println!("{}", fmt_default(fmt, pane_id)); }
    std::process::exit(0);
}

fn run_show_options(option_name: Option<String>) -> ! {
    match option_name.as_deref() {
        Some("prefix") => println!("prefix C-b"),
        Some("prefix2") => println!("prefix2 none"),
        Some("base-index") => println!("base-index 0"),
        Some("pane-base-index") => println!("pane-base-index 0"),
        Some("default-terminal") => println!("default-terminal \"screen-256color\""),
        Some(name) => println!("{} \"\"", name),
        None => {
            println!("prefix C-b");
            println!("base-index 0");
            println!("pane-base-index 0");
            println!("default-terminal \"screen-256color\"");
        }
    }
    std::process::exit(0);
}

fn run_capture_pane(target: Option<String>, print_stdout: bool) -> ! {
    let pane_id = target.as_deref()
        .map(|t| t.trim_start_matches('%').parse::<u32>().unwrap_or(0)).unwrap_or(0);
    let mut args = vec!["-t".into(), format!("%{}", pane_id)];
    if print_stdout { args.push("-p".into()); }
    match send_ipc_request(&IpcRequest { id: 1, command: "capture-pane".into(), args }) {
        Ok(resp) if resp.success && print_stdout => {
            if let Some(c) = resp.data.get("content").and_then(|v| v.as_str()) { print!("{}", c); }
        }
        _ => {}
    }
    std::process::exit(0);
}

fn run_new_session_or_window(print_info: bool, format: Option<String>) -> ! {
    let req = IpcRequest { id: 1, command: "split-window".into(), args: vec!["-h".into()] };
    match send_ipc_request(&req) {
        Ok(resp) if resp.success => {
            if print_info {
                let id_str = resp.data.get("pane_id").and_then(|v| v.as_str()).unwrap_or("%0");
                let id_num: u32 = id_str.trim_start_matches('%').parse().unwrap_or(0);
                println!("{}", format.as_deref().map(|f| fmt_default(f, id_num)).unwrap_or_else(|| id_str.into()));
            }
            std::process::exit(0);
        }
        Ok(resp) => { eprint_and_exit(resp.error); }
        Err(e) => { eprintln!("zwg: failed to connect to server: {}", e); std::process::exit(1); }
    }
}

fn run_list_windows(format: Option<String>) -> ! {
    if let Some(fmt) = &format { println!("{}", fmt_default(fmt, 0)); }
    else { println!("0: zwg* (1 panes)"); }
    std::process::exit(0);
}

fn run_split_window(
    horizontal: bool, start_dir: Option<&str>, command: &[String],
    print_info: bool, format: Option<String>,
) -> ! {
    let mut args = Vec::new();
    if horizontal { args.push("-h".into()); }
    if let Some(d) = start_dir { args.push("-c".into()); args.push(d.into()); }
    if !command.is_empty() { args.push("--".into()); args.extend_from_slice(command); }

    let req = IpcRequest { id: 1, command: "split-window".into(), args };
    match send_ipc_request(&req) {
        Ok(resp) if resp.success => {
            if print_info {
                let id_str = resp.data.get("pane_id").and_then(|v| v.as_str()).unwrap_or("%0");
                let id_num: u32 = id_str.trim_start_matches('%').parse().unwrap_or(0);
                println!("{}", format.as_deref().map(|f| fmt_default(f, id_num)).unwrap_or_else(|| id_str.into()));
            }
            std::process::exit(0);
        }
        Ok(resp) => { eprint_and_exit(resp.error); }
        Err(e) => { eprintln!("zwg: failed to connect to server: {}", e); std::process::exit(1); }
    }
}

fn run_ipc_command(command: &str, args: Vec<String>) -> ! {
    let req = IpcRequest { id: 1, command: command.into(), args };
    match send_ipc_request(&req) {
        Ok(resp) if resp.success => {
            if command == "split-window" {
                if let Some(id) = resp.data.get("pane_id").and_then(|v| v.as_str()) {
                    println!("{}", id);
                }
            }
            std::process::exit(0);
        }
        Ok(resp) => { eprint_and_exit(resp.error); }
        Err(e) => { eprintln!("zwg: failed to connect to server: {}", e); std::process::exit(1); }
    }
}

fn run_ipc_command_list_panes(args: Vec<String>) -> ! {
    let req = IpcRequest { id: 1, command: "list-panes".into(), args: args.clone() };
    match send_ipc_request(&req) {
        Ok(resp) if resp.success => {
            let custom_fmt = args.iter().position(|a| a == "-F")
                .and_then(|i| args.get(i + 1)).map(|s| s.as_str());
            if let Some(panes) = resp.data.get("panes").and_then(|v| v.as_array()) {
                for pane in panes {
                    let id = pane.get("pane_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let w = pane.get("width").and_then(|v| v.as_u64()).unwrap_or(120) as u16;
                    let h = pane.get("height").and_then(|v| v.as_u64()).unwrap_or(40) as u16;
                    let active = pane.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
                    if let Some(f) = custom_fmt {
                        println!("{}", expand_tmux_format(f, id, w, h, active));
                    } else {
                        println!("%{}: [{}x{}] %{}{}", id, w, h, id, if active { " (active)" } else { "" });
                    }
                }
            }
            std::process::exit(0);
        }
        Ok(resp) => { eprint_and_exit(resp.error); }
        Err(e) => { eprintln!("zwg: failed to connect to server: {}", e); std::process::exit(1); }
    }
}

fn eprint_and_exit(error: Option<String>) -> ! {
    eprintln!("zwg: {}", error.unwrap_or_else(|| "unknown error".into()));
    std::process::exit(1);
}

// ── IPC transport ─────────────────────────────────────────────────────────

fn send_ipc_request(request: &IpcRequest) -> anyhow::Result<IpcResponse> {
    send_ipc_named_pipe(request).or_else(|_| send_ipc_tcp(request))
}

fn send_ipc_named_pipe(request: &IpcRequest) -> anyhow::Result<IpcResponse> {
    let mut pipe = std::fs::OpenOptions::new().read(true).write(true)
        .open(r"\\.\pipe\zwg")
        .map_err(|e| anyhow::anyhow!("named pipe not available: {}", e))?;
    let json = serde_json::to_string(request)?;
    writeln!(pipe, "{}", json)?;
    pipe.flush()?;
    let mut reader = BufReader::new(pipe);
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 || line.trim().is_empty() {
        return Err(anyhow::anyhow!("no response from named pipe server"));
    }
    Ok(serde_json::from_str(line.trim())?)
}

fn send_ipc_tcp(request: &IpcRequest) -> anyhow::Result<IpcResponse> {
    use std::net::TcpStream;
    let mut stream = TcpStream::connect("127.0.0.1:51985")
        .map_err(|e| anyhow::anyhow!("cannot connect to zwg (is it running?): {}", e))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(20)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(20)))?;
    let json = serde_json::to_string(request)?;
    writeln!(stream, "{}", json)?;
    stream.flush()?;
    for line in BufReader::new(&stream).lines() {
        let line = line?;
        if line.is_empty() { continue; }
        return Ok(serde_json::from_str(&line)?);
    }
    Err(anyhow::anyhow!("no response from server"))
}
