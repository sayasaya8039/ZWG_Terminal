//! Ollama local LLM integration — streaming inference for terminal intelligence.
//!
//! Connects to a local Ollama server for:
//! - Real-time command prediction (streaming /api/generate)
//! - Semantic embeddings (/api/embed)
//! - Output summarization via LLM

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::time::Duration;

/// Default Ollama server URL.
const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Connection timeout for health checks.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// Generation timeout (max wait for full response).
const GENERATE_TIMEOUT: Duration = Duration::from_secs(5);

/// Prompt template for command prediction.
const PREDICT_PROMPT_TEMPLATE: &str = "\
You are a shell command predictor. Given the user's partial input, working directory, \
and recent command history, predict the most likely complete command. \
Output ONLY the command, nothing else.\n\n\
Working directory: {cwd}\n\
Recent commands:\n{history}\n\n\
Partial input: {input}\n\
Predicted command:";

/// Client for communicating with a local Ollama server.
pub struct OllamaClient {
    base_url: String,
}

/// Request body for /api/generate.
#[derive(Debug, Clone, Serialize)]
pub struct GenerateRequest {
    pub model: String,
    pub prompt: String,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<GenerateOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<String>,
}

/// Optional generation parameters.
#[derive(Debug, Clone, Serialize)]
pub struct GenerateOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

/// Model information from /api/tags.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub parameter_size: Option<String>,
    #[serde(default)]
    pub quantization_level: Option<String>,
}

/// Single line from streaming /api/generate response.
#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default)]
    response: String,
    #[serde(default)]
    done: bool,
}

/// Response envelope from /api/tags.
#[derive(Debug, Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagsModel>,
}

/// Model entry within /api/tags response.
#[derive(Debug, Deserialize)]
struct TagsModel {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    details: Option<TagsModelDetails>,
}

#[derive(Debug, Deserialize)]
struct TagsModelDetails {
    family: Option<String>,
    parameter_size: Option<String>,
    quantization_level: Option<String>,
}

/// Response from /api/embed.
#[derive(Debug, Deserialize)]
struct EmbedResponse {
    #[serde(default)]
    embeddings: Vec<Vec<f32>>,
}

/// Non-streaming /api/generate response.
#[derive(Debug, Deserialize)]
struct GenerateResponse {
    #[serde(default)]
    response: String,
}

/// Helper: POST JSON and deserialize the response.
fn post_json<T: serde::de::DeserializeOwned>(
    agent: &ureq::Agent,
    url: &str,
    payload: &impl Serialize,
) -> Result<T> {
    let json_bytes = serde_json::to_vec(payload)
        .map_err(|e| anyhow!("failed to serialize request: {e}"))?;
    let resp_str = agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_bytes(&json_bytes)
        .map_err(|e| anyhow!("HTTP POST failed: {e}"))?
        .into_string()
        .map_err(|e| anyhow!("failed to read response body: {e}"))?;
    serde_json::from_str(&resp_str)
        .map_err(|e| anyhow!("failed to parse response JSON: {e}"))
}

/// Helper: POST JSON and return the raw response for streaming.
fn post_json_raw(
    agent: &ureq::Agent,
    url: &str,
    payload: &impl Serialize,
) -> Result<ureq::Response> {
    let json_bytes = serde_json::to_vec(payload)
        .map_err(|e| anyhow!("failed to serialize request: {e}"))?;
    let response = agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_bytes(&json_bytes)
        .map_err(|e| anyhow!("HTTP POST failed: {e}"))?;
    Ok(response)
}

impl OllamaClient {
    /// Create a new client. Uses `http://localhost:11434` when `base_url` is `None`.
    pub fn new(base_url: Option<&str>) -> Self {
        let url = base_url
            .unwrap_or(DEFAULT_BASE_URL)
            .trim_end_matches('/')
            .to_string();
        Self { base_url: url }
    }

    /// Check whether the Ollama server is reachable (GET /api/tags with 1s timeout).
    pub fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(CONNECT_TIMEOUT)
            .build();
        agent.get(&url).call().is_ok()
    }

    /// List models available on the Ollama server.
    pub fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/api/tags", self.base_url);
        let resp_str = ureq::get(&url)
            .call()
            .map_err(|e| anyhow!("Ollama /api/tags request failed: {e}"))?
            .body_mut()
            .read_to_string()
            .map_err(|e| anyhow!("failed to read /api/tags response: {e}"))?;
        let resp: TagsResponse = serde_json::from_str(&resp_str)
            .map_err(|e| anyhow!("failed to parse /api/tags response: {e}"))?;

        let models = resp
            .models
            .into_iter()
            .map(|m| {
                let (family, parameter_size, quantization_level) = match m.details {
                    Some(d) => (d.family, d.parameter_size, d.quantization_level),
                    None => (None, None, None),
                };
                ModelInfo {
                    name: m.name,
                    size: m.size,
                    family,
                    parameter_size,
                    quantization_level,
                }
            })
            .collect();
        Ok(models)
    }

    /// Non-streaming generation: POST /api/generate with stream=false.
    pub fn generate(&self, req: &GenerateRequest) -> Result<String> {
        let url = format!("{}/api/generate", self.base_url);

        let body = GenerateRequest {
            stream: false,
            ..req.clone()
        };

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(GENERATE_TIMEOUT)
            .build();

        let resp: GenerateResponse = post_json(&agent, &url, &body)?;
        Ok(resp.response)
    }

    /// Streaming generation: POST /api/generate with stream=true.
    /// Calls `callback` with each token as it arrives.
    pub fn generate_stream(
        &self,
        req: &GenerateRequest,
        mut callback: impl FnMut(&str),
    ) -> Result<()> {
        let url = format!("{}/api/generate", self.base_url);

        let body = GenerateRequest {
            stream: true,
            ..req.clone()
        };

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .build();

        let resp = post_json_raw(&agent, &url, &body)?;
        let reader = std::io::BufReader::new(resp.into_reader());
        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(_) => break,
            };
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<StreamChunk>(&line) {
                Ok(chunk) => {
                    if !chunk.response.is_empty() {
                        callback(&chunk.response);
                    }
                    if chunk.done {
                        break;
                    }
                }
                Err(_) => continue,
            }
        }

        Ok(())
    }

    /// Generate embeddings via POST /api/embed.
    pub fn embed(&self, model: &str, input: &str) -> Result<Vec<f32>> {
        let url = format!("{}/api/embed", self.base_url);

        let payload = serde_json::json!({
            "model": model,
            "input": input,
        });

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(GENERATE_TIMEOUT)
            .build();

        let resp: EmbedResponse = post_json(&agent, &url, &payload)?;
        resp.embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("empty embeddings response"))
    }

    /// High-level command prediction wrapper.
    ///
    /// Builds a prompt from partial input, command history, and cwd,
    /// then asks the LLM for a single predicted command.
    pub fn predict_command(
        &self,
        input: &str,
        history: &[&str],
        cwd: &str,
    ) -> Result<Option<String>> {
        if input.trim().is_empty() {
            return Ok(None);
        }

        let history_text = if history.is_empty() {
            "(none)".to_string()
        } else {
            history.iter().map(|h| format!("  {h}")).collect::<Vec<_>>().join("\n")
        };

        let prompt = PREDICT_PROMPT_TEMPLATE
            .replace("{cwd}", cwd)
            .replace("{history}", &history_text)
            .replace("{input}", input);

        let req = GenerateRequest {
            model: "qwen3:0.6b".to_string(),
            prompt,
            stream: false,
            options: Some(GenerateOptions {
                temperature: Some(0.1),
                top_k: Some(10),
                top_p: Some(0.9),
                num_predict: Some(100),
                stop: Some(vec!["\n".to_string()]),
            }),
            keep_alive: Some("30m".to_string()),
        };

        let response = self.generate(&req)?;
        let trimmed = response.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            Ok(Some(trimmed.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_creation_default_url() {
        let client = OllamaClient::new(None);
        assert_eq!(client.base_url, "http://localhost:11434");
    }

    #[test]
    fn client_creation_custom_url() {
        let client = OllamaClient::new(Some("http://myhost:8080/"));
        assert_eq!(client.base_url, "http://myhost:8080");
    }

    #[test]
    fn generate_request_serialization() {
        let req = GenerateRequest {
            model: "qwen3:0.6b".to_string(),
            prompt: "hello".to_string(),
            stream: false,
            options: Some(GenerateOptions {
                temperature: Some(0.5),
                top_k: None,
                top_p: None,
                num_predict: Some(50),
                stop: None,
            }),
            keep_alive: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"model\":\"qwen3:0.6b\""));
        assert!(json.contains("\"temperature\":0.5"));
        assert!(!json.contains("\"keep_alive\""));
    }

    #[test]
    fn stream_chunk_parsing() {
        let line = r#"{"model":"qwen3:0.6b","response":" ls","done":false}"#;
        let chunk: StreamChunk = serde_json::from_str(line).unwrap();
        assert_eq!(chunk.response, " ls");
        assert!(!chunk.done);

        let done_line = r#"{"model":"qwen3:0.6b","response":"","done":true,"total_duration":123456}"#;
        let done_chunk: StreamChunk = serde_json::from_str(done_line).unwrap();
        assert!(done_chunk.done);
        assert!(done_chunk.response.is_empty());
    }

    #[test]
    fn unavailable_server_returns_false() {
        // Port 1 should never have Ollama running
        let client = OllamaClient::new(Some("http://127.0.0.1:1"));
        assert!(!client.is_available());
    }

    #[test]
    fn predict_command_empty_input() {
        let client = OllamaClient::new(Some("http://127.0.0.1:1"));
        let result = client.predict_command("", &[], "/tmp");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
