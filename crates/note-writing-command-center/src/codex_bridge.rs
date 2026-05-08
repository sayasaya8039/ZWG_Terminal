use crate::model::{Article, AssistantTask, build_assistant_prompt, local_assistant_response};
use anyhow::{Context, anyhow};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

#[derive(Debug)]
pub struct AssistantAnswer {
    pub text: String,
    pub source: &'static str,
    pub error: Option<String>,
}

pub fn ask_assistant(task: AssistantTask, article: Article) -> AssistantAnswer {
    let prompt = build_assistant_prompt(task, &article);
    match ask_codex_app_server(&prompt) {
        Ok(text) if !text.trim().is_empty() => AssistantAnswer {
            text,
            source: "codex",
            error: None,
        },
        Ok(_) => AssistantAnswer {
            text: local_assistant_response(task, &article),
            source: "local",
            error: Some(
                "Codexから空の応答が返りました。ローカル支援に切り替えました。".to_string(),
            ),
        },
        Err(err) => AssistantAnswer {
            text: local_assistant_response(task, &article),
            source: "local",
            error: Some(format!("{err:#}")),
        },
    }
}

fn ask_codex_app_server(prompt: &str) -> anyhow::Result<String> {
    let child = Command::new("codex")
        .args(["app-server", "--listen", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("codex app-serverを起動できませんでした")?;
    let mut server = AppServerProcess { child };

    let stdout = server
        .child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("codex app-server stdoutを取得できませんでした"))?;
    let stderr = server.child.stderr.take();
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(value);
            }
        }
    });

    if let Some(stderr) = stderr {
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for _line in reader.lines().map_while(Result::ok) {}
        });
    }

    let stdin = server
        .child
        .stdin
        .as_mut()
        .ok_or_else(|| anyhow!("codex app-server stdinを取得できませんでした"))?;

    send_request(
        stdin,
        1,
        "initialize",
        json!({
            "clientInfo": {
                "name": "note_writing_command_center_native",
                "title": "note執筆司令塔",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "experimentalApi": true
            }
        }),
    )?;
    wait_for_response(&rx, 1)?;

    send_request(
        stdin,
        2,
        "thread/start",
        json!({
            "cwd": std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")).to_string_lossy(),
            "approvalPolicy": "never",
            "sandbox": "read-only",
            "serviceName": "note_writing_command_center_native",
            "ephemeral": true,
            "experimentalRawEvents": false,
            "persistExtendedHistory": false
        }),
    )?;
    let thread_response = wait_for_response(&rx, 2)?;
    let thread_id = thread_response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("thread/start応答にthread idがありません"))?
        .to_string();

    send_request(
        stdin,
        3,
        "turn/start",
        json!({
            "threadId": thread_id,
            "input": [{
                "type": "text",
                "text": prompt,
                "text_elements": []
            }],
            "approvalPolicy": "never",
            "sandboxPolicy": {
                "type": "readOnly",
                "networkAccess": false
            }
        }),
    )?;
    let turn_response = wait_for_response(&rx, 3)?;
    let turn_id = turn_response
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("turn/start応答にturn idがありません"))?
        .to_string();

    let answer = wait_for_turn(&rx, &thread_id, &turn_id)?;
    server.shutdown();
    Ok(answer)
}

fn send_request(
    stdin: &mut impl Write,
    id: u64,
    method: &str,
    params: Value,
) -> anyhow::Result<()> {
    let request = json!({
        "id": id,
        "method": method,
        "params": params,
    });
    writeln!(stdin, "{request}")?;
    stdin.flush()?;
    Ok(())
}

fn wait_for_response(rx: &Receiver<Value>, id: u64) -> anyhow::Result<Value> {
    loop {
        let value = rx
            .recv_timeout(Duration::from_secs(45))
            .with_context(|| format!("codex app-server応答がタイムアウトしました: id={id}"))?;
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(anyhow!("codex app-serverエラー: {error}"));
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn wait_for_turn(rx: &Receiver<Value>, thread_id: &str, turn_id: &str) -> anyhow::Result<String> {
    let mut answer = String::new();
    loop {
        let value = rx
            .recv_timeout(Duration::from_secs(120))
            .context("Codexの応答がタイムアウトしました")?;
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = value.get("params").unwrap_or(&Value::Null);

        match method {
            "item/agentMessage/delta"
                if params.get("threadId").and_then(Value::as_str) == Some(thread_id)
                    && params.get("turnId").and_then(Value::as_str) == Some(turn_id) =>
            {
                if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                    answer.push_str(delta);
                }
            }
            "item/completed"
                if params.get("threadId").and_then(Value::as_str) == Some(thread_id)
                    && params.get("turnId").and_then(Value::as_str) == Some(turn_id) =>
            {
                if params.pointer("/item/type").and_then(Value::as_str) == Some("agentMessage")
                    && answer.trim().is_empty()
                {
                    if let Some(text) = params.pointer("/item/text").and_then(Value::as_str) {
                        answer.push_str(text);
                    }
                }
            }
            "turn/completed"
                if params.get("threadId").and_then(Value::as_str) == Some(thread_id)
                    && params.pointer("/turn/id").and_then(Value::as_str) == Some(turn_id) =>
            {
                return Ok(answer.trim().to_string());
            }
            _ => {}
        }
    }
}

struct AppServerProcess {
    child: Child,
}

impl AppServerProcess {
    fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for AppServerProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}
