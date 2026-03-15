use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

pub const ANTHROPIC_API_ENV: &str = "ANTHROPIC_API_KEY";
pub const OPENAI_API_ENV: &str = "OPENAI_API_KEY";
pub const GEMINI_API_ENV: &str = "GEMINI_API_KEY";
pub const ANTHROPIC_MODEL_ENV: &str = "ZWG_CLAUDE_MODEL";
pub const OPENAI_MODEL_ENV: &str = "ZWG_OPENAI_MODEL";
pub const GEMINI_MODEL_ENV: &str = "ZWG_GEMINI_MODEL";
pub const ANTHROPIC_BASE_URL_ENV: &str = "ZWG_ANTHROPIC_BASE_URL";
pub const OPENAI_BASE_URL_ENV: &str = "ZWG_OPENAI_BASE_URL";
pub const GEMINI_BASE_URL_ENV: &str = "ZWG_GEMINI_BASE_URL";
pub const QUERY_MIN_CHARS: usize = 3;

const MAX_SUGGESTIONS: usize = 3;
const MAX_QUERY_CHARS: usize = 240;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";
const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";
const DEFAULT_OPENAI_MODEL: &str = "gpt-5-mini";
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
const AI_SYSTEM_PROMPT: &str = concat!(
    "You generate short Windows terminal command suggestions. ",
    "Return strict JSON only. ",
    "Format as an array with up to 3 objects using keys title, command, detail. ",
    "Prefer PowerShell or shell-agnostic commands, keep each command copy-pastable, ",
    "avoid destructive or privileged operations, and do not include commentary outside JSON."
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProvider {
    Anthropic,
    OpenAi,
    Gemini,
}

impl AiProvider {
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Claude",
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Gemini",
        }
    }

    pub fn api_env(self) -> &'static str {
        match self {
            Self::Anthropic => ANTHROPIC_API_ENV,
            Self::OpenAi => OPENAI_API_ENV,
            Self::Gemini => GEMINI_API_ENV,
        }
    }

    pub fn model_env(self) -> &'static str {
        match self {
            Self::Anthropic => ANTHROPIC_MODEL_ENV,
            Self::OpenAi => OPENAI_MODEL_ENV,
            Self::Gemini => GEMINI_MODEL_ENV,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiSuggestion {
    pub title: String,
    pub command: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiSuggestionStatus {
    Idle,
    Loading,
    Ready,
    Disabled,
    MissingApiKey,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiQueryRequest {
    pub request_id: u64,
    pub query: String,
    pub provider: AiProvider,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct AiSuggestionState {
    status: AiSuggestionStatus,
    suggestions: Vec<AiSuggestion>,
    active_request_id: u64,
}

impl Default for AiSuggestionState {
    fn default() -> Self {
        Self {
            status: AiSuggestionStatus::Idle,
            suggestions: Vec::new(),
            active_request_id: 0,
        }
    }
}

impl AiSuggestionState {
    pub fn status(&self) -> &AiSuggestionStatus {
        &self.status
    }

    pub fn suggestions(&self) -> &[AiSuggestion] {
        &self.suggestions
    }

    pub fn begin_request(
        &mut self,
        query: &str,
        enabled: bool,
        api_key_present: bool,
        provider: AiProvider,
        model: &str,
    ) -> Option<AiQueryRequest> {
        self.active_request_id = self.active_request_id.saturating_add(1);
        let request_id = self.active_request_id;
        let query = normalize_query(query);
        self.suggestions.clear();

        if !enabled {
            self.status = AiSuggestionStatus::Disabled;
            return None;
        }
        if query.chars().count() < QUERY_MIN_CHARS {
            self.status = AiSuggestionStatus::Idle;
            return None;
        }
        if !api_key_present {
            self.status = AiSuggestionStatus::MissingApiKey;
            return None;
        }

        self.status = AiSuggestionStatus::Loading;
        Some(AiQueryRequest {
            request_id,
            query,
            provider,
            model: resolve_ai_model(provider, model),
        })
    }

    pub fn apply_result(
        &mut self,
        request_id: u64,
        result: Result<Vec<AiSuggestion>, String>,
    ) -> bool {
        if request_id != self.active_request_id {
            return false;
        }

        match result {
            Ok(items) => {
                self.suggestions = sanitize_suggestions(items);
                self.status = AiSuggestionStatus::Ready;
            }
            Err(error) => {
                self.suggestions.clear();
                self.status = AiSuggestionStatus::Error(error);
            }
        }
        true
    }
}

pub fn sanitize_ai_provider(raw: &str) -> AiProvider {
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "openai" | "open-ai" => AiProvider::OpenAi,
        "gemini" | "google" => AiProvider::Gemini,
        _ => AiProvider::Anthropic,
    }
}

pub fn sanitize_ai_provider_config_value(raw: &str) -> String {
    sanitize_ai_provider(raw).config_value().to_string()
}

pub fn next_ai_provider(provider: AiProvider) -> AiProvider {
    match provider {
        AiProvider::Anthropic => AiProvider::OpenAi,
        AiProvider::OpenAi => AiProvider::Gemini,
        AiProvider::Gemini => AiProvider::Anthropic,
    }
}

pub fn default_model_for_provider(provider: AiProvider) -> &'static str {
    match provider {
        AiProvider::Anthropic => DEFAULT_ANTHROPIC_MODEL,
        AiProvider::OpenAi => DEFAULT_OPENAI_MODEL,
        AiProvider::Gemini => DEFAULT_GEMINI_MODEL,
    }
}

pub fn resolve_ai_api_key(provider: AiProvider, configured: &str) -> Option<String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        return Some(configured.to_string());
    }

    std::env::var(provider.api_env())
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn resolve_ai_model(provider: AiProvider, configured: &str) -> String {
    let env_model = std::env::var(provider.model_env()).ok();
    let candidate = env_model.as_deref().unwrap_or(configured).trim();
    if candidate.is_empty() {
        default_model_for_provider(provider).to_string()
    } else {
        candidate.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct AiDirectClient {
    anthropic_base_url: String,
    openai_base_url: String,
    gemini_base_url: String,
}

impl Default for AiDirectClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AiDirectClient {
    pub fn new() -> Self {
        Self {
            anthropic_base_url: resolve_base_url(
                ANTHROPIC_BASE_URL_ENV,
                DEFAULT_ANTHROPIC_BASE_URL,
            ),
            openai_base_url: resolve_base_url(OPENAI_BASE_URL_ENV, DEFAULT_OPENAI_BASE_URL),
            gemini_base_url: resolve_base_url(GEMINI_BASE_URL_ENV, DEFAULT_GEMINI_BASE_URL),
        }
    }

    pub fn fetch_suggestions(
        &self,
        provider: AiProvider,
        api_key: &str,
        query: &str,
        model: &str,
    ) -> Result<Vec<AiSuggestion>, String> {
        match provider {
            AiProvider::Anthropic => self.fetch_anthropic_suggestions(api_key, query, model),
            AiProvider::OpenAi => self.fetch_openai_suggestions(api_key, query, model),
            AiProvider::Gemini => self.fetch_gemini_suggestions(api_key, query, model),
        }
    }

    fn fetch_anthropic_suggestions(
        &self,
        api_key: &str,
        query: &str,
        model: &str,
    ) -> Result<Vec<AiSuggestion>, String> {
        let url = format!(
            "{}/v1/messages",
            self.anthropic_base_url.trim_end_matches('/')
        );
        let body = build_anthropic_request_body(query, model);
        let agent = build_http_agent();
        let request = agent
            .post(&url)
            .set("x-api-key", api_key)
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("content-type", "application/json");

        match request.send_json(body) {
            Ok(response) => response
                .into_string()
                .map_err(|error| format!("Claude response read failed: {error}"))
                .and_then(|body| parse_anthropic_suggestions_from_response(&body)),
            Err(ureq::Error::Status(_, response)) => response
                .into_string()
                .map_err(|error| format!("Claude error response read failed: {error}"))
                .and_then(|body| Err(parse_api_error_body(AiProvider::Anthropic, &body))),
            Err(ureq::Error::Transport(error)) => Err(format!("Claude request failed: {error}")),
        }
    }

    fn fetch_openai_suggestions(
        &self,
        api_key: &str,
        query: &str,
        model: &str,
    ) -> Result<Vec<AiSuggestion>, String> {
        let url = format!(
            "{}/v1/chat/completions",
            self.openai_base_url.trim_end_matches('/')
        );
        let body = build_openai_request_body(query, model);
        let agent = build_http_agent();
        let request = agent
            .post(&url)
            .set("authorization", &format!("Bearer {api_key}"))
            .set("content-type", "application/json");

        match request.send_json(body) {
            Ok(response) => response
                .into_string()
                .map_err(|error| format!("OpenAI response read failed: {error}"))
                .and_then(|body| parse_openai_suggestions_from_response(&body)),
            Err(ureq::Error::Status(_, response)) => response
                .into_string()
                .map_err(|error| format!("OpenAI error response read failed: {error}"))
                .and_then(|body| Err(parse_api_error_body(AiProvider::OpenAi, &body))),
            Err(ureq::Error::Transport(error)) => Err(format!("OpenAI request failed: {error}")),
        }
    }

    fn fetch_gemini_suggestions(
        &self,
        api_key: &str,
        query: &str,
        model: &str,
    ) -> Result<Vec<AiSuggestion>, String> {
        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.gemini_base_url.trim_end_matches('/'),
            model.trim(),
            api_key.trim()
        );
        let body = build_gemini_request_body(query);
        let agent = build_http_agent();
        let request = agent.post(&url).set("content-type", "application/json");

        match request.send_json(body) {
            Ok(response) => response
                .into_string()
                .map_err(|error| format!("Gemini response read failed: {error}"))
                .and_then(|body| parse_gemini_suggestions_from_response(&body)),
            Err(ureq::Error::Status(_, response)) => response
                .into_string()
                .map_err(|error| format!("Gemini error response read failed: {error}"))
                .and_then(|body| Err(parse_api_error_body(AiProvider::Gemini, &body))),
            Err(ureq::Error::Transport(error)) => Err(format!("Gemini request failed: {error}")),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ClaudeMessageRequest {
    model: String,
    max_tokens: u16,
    system: String,
    messages: Vec<ClaudeMessage>,
}

#[derive(Debug, Clone, Serialize)]
struct ClaudeMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerateRequest {
    system_instruction: GeminiInstruction,
    contents: Vec<GeminiContentRequest>,
    generation_config: GeminiGenerationConfig,
}

#[derive(Debug, Clone, Serialize)]
struct GeminiInstruction {
    parts: Vec<GeminiTextPart>,
}

#[derive(Debug, Clone, Serialize)]
struct GeminiContentRequest {
    role: String,
    parts: Vec<GeminiTextPart>,
}

#[derive(Debug, Clone, Serialize)]
struct GeminiTextPart {
    text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    temperature: f32,
}

fn build_anthropic_request_body(query: &str, model: &str) -> ClaudeMessageRequest {
    ClaudeMessageRequest {
        model: resolve_ai_model(AiProvider::Anthropic, model),
        max_tokens: 256,
        system: AI_SYSTEM_PROMPT.to_string(),
        messages: vec![ClaudeMessage {
            role: "user".into(),
            content: prompt_for_query(query),
        }],
    }
}

fn build_openai_request_body(query: &str, model: &str) -> OpenAiChatRequest {
    OpenAiChatRequest {
        model: resolve_ai_model(AiProvider::OpenAi, model),
        messages: vec![
            OpenAiMessage {
                role: "system".into(),
                content: AI_SYSTEM_PROMPT.to_string(),
            },
            OpenAiMessage {
                role: "user".into(),
                content: prompt_for_query(query),
            },
        ],
    }
}

fn build_gemini_request_body(query: &str) -> GeminiGenerateRequest {
    GeminiGenerateRequest {
        system_instruction: GeminiInstruction {
            parts: vec![GeminiTextPart {
                text: AI_SYSTEM_PROMPT.to_string(),
            }],
        },
        contents: vec![GeminiContentRequest {
            role: "user".into(),
            parts: vec![GeminiTextPart {
                text: prompt_for_query(query),
            }],
        }],
        generation_config: GeminiGenerationConfig { temperature: 0.2 },
    }
}

fn prompt_for_query(query: &str) -> String {
    format!(
        "User query: {query}\nReturn only JSON. Example: \
[{{\"title\":\"List files\",\"command\":\"Get-ChildItem\",\"detail\":\"List the current directory\"}}]"
    )
}

pub fn parse_anthropic_suggestions_from_response(body: &str) -> Result<Vec<AiSuggestion>, String> {
    let response: ClaudeApiResponse = serde_json::from_str(body)
        .map_err(|error| format!("Claude response JSON parse failed: {error}"))?;

    if let Some(error) = response.error {
        return Err(error.message);
    }

    let joined_text = response
        .content
        .iter()
        .filter(|block| block.kind == "text")
        .filter_map(|block| block.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n");

    parse_suggestions_from_text("Claude", &joined_text)
}

pub fn parse_openai_suggestions_from_response(body: &str) -> Result<Vec<AiSuggestion>, String> {
    let response: OpenAiChatResponse = serde_json::from_str(body)
        .map_err(|error| format!("OpenAI response JSON parse failed: {error}"))?;

    if let Some(error) = response.error {
        return Err(error.message);
    }

    let joined_text = response
        .choices
        .iter()
        .filter_map(|choice| extract_openai_message_text(&choice.message.content))
        .collect::<Vec<_>>()
        .join("\n");

    parse_suggestions_from_text("OpenAI", &joined_text)
}

pub fn parse_gemini_suggestions_from_response(body: &str) -> Result<Vec<AiSuggestion>, String> {
    let response: GeminiApiResponse = serde_json::from_str(body)
        .map_err(|error| format!("Gemini response JSON parse failed: {error}"))?;

    if let Some(error) = response.error {
        return Err(error.message);
    }

    let joined_text = response
        .candidates
        .iter()
        .filter_map(|candidate| candidate.content.as_ref())
        .flat_map(|content| content.parts.iter())
        .filter_map(|part| part.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n");

    parse_suggestions_from_text("Gemini", &joined_text)
}

#[derive(Debug, Deserialize)]
struct ClaudeApiResponse {
    #[serde(default)]
    content: Vec<ClaudeContentBlock>,
    #[serde(default)]
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ClaudeContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseMessage {
    #[serde(default)]
    content: Value,
}

#[derive(Debug, Deserialize)]
struct GeminiApiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiResponseContent>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponseContent {
    #[serde(default)]
    parts: Vec<GeminiResponsePart>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponsePart {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    message: String,
}

#[derive(Debug, Deserialize)]
struct RawSuggestion {
    title: String,
    command: String,
    detail: String,
}

fn build_http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build()
}

fn resolve_base_url(env_name: &str, default_url: &str) -> String {
    std::env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default_url.to_string())
}

fn normalize_query(query: &str) -> String {
    query.trim().chars().take(MAX_QUERY_CHARS).collect()
}

fn parse_suggestions_from_text(source: &str, text: &str) -> Result<Vec<AiSuggestion>, String> {
    let json = extract_json_array(text)
        .ok_or_else(|| format!("{source} response did not contain a JSON array"))?;
    let raw_items: Vec<RawSuggestion> = serde_json::from_str(&json)
        .map_err(|error| format!("{source} suggestion JSON parse failed: {error}"))?;

    Ok(sanitize_suggestions(
        raw_items
            .into_iter()
            .map(|item| AiSuggestion {
                title: item.title.trim().to_string(),
                command: item.command.trim().to_string(),
                detail: item.detail.trim().to_string(),
            })
            .collect(),
    ))
}

fn extract_openai_message_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

fn sanitize_suggestions(items: Vec<AiSuggestion>) -> Vec<AiSuggestion> {
    let mut seen = HashSet::new();
    let mut filtered = Vec::new();

    for item in items {
        let title = item.title.trim();
        let command = item.command.trim();
        let detail = item.detail.trim();
        if title.is_empty() || command.is_empty() || detail.is_empty() {
            continue;
        }
        if is_dangerous_command(command) {
            continue;
        }
        let dedupe_key = command.to_ascii_lowercase();
        if seen.insert(dedupe_key) {
            filtered.push(AiSuggestion {
                title: title.to_string(),
                command: command.to_string(),
                detail: detail.to_string(),
            });
        }
        if filtered.len() >= MAX_SUGGESTIONS {
            break;
        }
    }

    filtered
}

fn is_dangerous_command(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    let blocked_patterns = [
        "rm -rf",
        "remove-item -recurse -force",
        "del /f /q",
        "format ",
        "diskpart",
        "shutdown ",
        "restart-computer",
        "stop-computer",
        "sc delete",
        "reg delete",
        "net user ",
    ];
    blocked_patterns
        .iter()
        .any(|pattern| lowered.contains(pattern))
}

fn extract_json_array(text: &str) -> Option<String> {
    if let Some(fenced) = extract_fenced_json(text) {
        return Some(fenced);
    }

    let chars: Vec<char> = text.chars().collect();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '[' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            ']' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = start {
                        return Some(chars[start..=index].iter().collect());
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn extract_fenced_json(text: &str) -> Option<String> {
    let fence_start = text.find("```")?;
    let rest = &text[fence_start + 3..];
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let fence_end = rest.find("```")?;
    Some(rest[..fence_end].trim().to_string())
}

fn parse_api_error_body(provider: AiProvider, body: &str) -> String {
    serde_json::from_str::<GenericApiErrorEnvelope>(body)
        .ok()
        .and_then(|response| response.error.map(|error| error.message))
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| format!("{} API returned an error: {body}", provider.label()))
}

#[derive(Debug, Deserialize)]
struct GenericApiErrorEnvelope {
    #[serde(default)]
    error: Option<ApiErrorBody>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestion(title: &str, command: &str, detail: &str) -> AiSuggestion {
        AiSuggestion {
            title: title.into(),
            command: command.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn sanitize_ai_provider_defaults_to_anthropic() {
        assert_eq!(sanitize_ai_provider("openai"), AiProvider::OpenAi);
        assert_eq!(sanitize_ai_provider("gemini"), AiProvider::Gemini);
        assert_eq!(sanitize_ai_provider("unknown"), AiProvider::Anthropic);
    }

    #[test]
    fn resolve_ai_model_uses_provider_defaults() {
        assert_eq!(
            resolve_ai_model(AiProvider::Anthropic, ""),
            DEFAULT_ANTHROPIC_MODEL
        );
        assert_eq!(
            resolve_ai_model(AiProvider::OpenAi, ""),
            DEFAULT_OPENAI_MODEL
        );
        assert_eq!(
            resolve_ai_model(AiProvider::Gemini, ""),
            DEFAULT_GEMINI_MODEL
        );
    }

    #[test]
    fn short_query_does_not_start_request() {
        let mut state = AiSuggestionState::default();

        let request = state.begin_request("gi", true, true, AiProvider::Anthropic, "");

        assert!(request.is_none());
        assert_eq!(state.status(), &AiSuggestionStatus::Idle);
        assert!(state.suggestions().is_empty());
    }

    #[test]
    fn missing_api_key_surfaces_explicit_status() {
        let mut state = AiSuggestionState::default();

        let request = state.begin_request("git log", true, false, AiProvider::OpenAi, "");

        assert!(request.is_none());
        assert_eq!(state.status(), &AiSuggestionStatus::MissingApiKey);
    }

    #[test]
    fn begin_request_marks_state_loading() {
        let mut state = AiSuggestionState::default();

        let request = state
            .begin_request(" git status ", true, true, AiProvider::Gemini, "")
            .expect("request");

        assert_eq!(request.request_id, 1);
        assert_eq!(request.query, "git status");
        assert_eq!(request.provider, AiProvider::Gemini);
        assert_eq!(request.model, DEFAULT_GEMINI_MODEL);
        assert_eq!(state.status(), &AiSuggestionStatus::Loading);
    }

    #[test]
    fn stale_results_are_ignored() {
        let mut state = AiSuggestionState::default();
        let first = state
            .begin_request("git status", true, true, AiProvider::Anthropic, "")
            .expect("first");
        let second = state
            .begin_request("git diff", true, true, AiProvider::OpenAi, "")
            .expect("second");

        let applied = state.apply_result(
            first.request_id,
            Ok(vec![suggestion("Status", "git status", "show status")]),
        );

        assert!(!applied);
        assert_eq!(state.status(), &AiSuggestionStatus::Loading);
        assert_eq!(state.active_request_id, second.request_id);
    }

    #[test]
    fn successful_result_deduplicates_filters_and_limits_items() {
        let mut state = AiSuggestionState::default();
        let request = state
            .begin_request(
                "git clean up branches",
                true,
                true,
                AiProvider::Anthropic,
                "",
            )
            .expect("request");

        let applied = state.apply_result(
            request.request_id,
            Ok(vec![
                suggestion("Status", "git status", "show branch state"),
                suggestion("Status", "git status", "duplicate"),
                suggestion("Danger", "rm -rf .", "dangerous"),
                suggestion("Fetch", "git fetch --prune", "sync refs"),
                suggestion(
                    "Prune",
                    "git branch --merged | ForEach-Object { $_ }",
                    "list merged",
                ),
                suggestion("Extra", "git remote -v", "show remotes"),
            ]),
        );

        assert!(applied);
        assert_eq!(state.status(), &AiSuggestionStatus::Ready);
        assert_eq!(state.suggestions().len(), 3);
        assert_eq!(state.suggestions()[0].command, "git status");
        assert_eq!(state.suggestions()[1].command, "git fetch --prune");
        assert_eq!(
            state.suggestions()[2].command,
            "git branch --merged | ForEach-Object { $_ }"
        );
    }

    #[test]
    fn failed_result_sets_error_status() {
        let mut state = AiSuggestionState::default();
        let request = state
            .begin_request("git stash", true, true, AiProvider::Anthropic, "")
            .expect("request");

        let applied = state.apply_result(request.request_id, Err("network failed".into()));

        assert!(applied);
        assert_eq!(
            state.status(),
            &AiSuggestionStatus::Error("network failed".into())
        );
        assert!(state.suggestions().is_empty());
    }

    #[test]
    fn anthropic_response_parser_extracts_json_from_text_block() {
        let body = r#"{"content":[{"type":"text","text":"```json\n[{\"title\":\"List\",\"command\":\"Get-ChildItem\",\"detail\":\"list current directory\"}]\n```"}]}"#;

        let parsed = parse_anthropic_suggestions_from_response(body).expect("parsed");

        assert_eq!(
            parsed,
            vec![suggestion(
                "List",
                "Get-ChildItem",
                "list current directory"
            )]
        );
    }

    #[test]
    fn openai_response_parser_extracts_json_from_message() {
        let body = r#"{"choices":[{"message":{"content":"[{\"title\":\"Status\",\"command\":\"git status\",\"detail\":\"show repository status\"}]"}}]}"#;

        let parsed = parse_openai_suggestions_from_response(body).expect("parsed");

        assert_eq!(
            parsed,
            vec![suggestion("Status", "git status", "show repository status")]
        );
    }

    #[test]
    fn gemini_response_parser_extracts_json_from_candidate() {
        let body = r#"{"candidates":[{"content":{"parts":[{"text":"[{\"title\":\"Fetch\",\"command\":\"git fetch --all\",\"detail\":\"fetch every remote\"}]"}]}}]}"#;

        let parsed = parse_gemini_suggestions_from_response(body).expect("parsed");

        assert_eq!(
            parsed,
            vec![suggestion("Fetch", "git fetch --all", "fetch every remote")]
        );
    }

    #[test]
    fn provider_specific_defaults_are_stable() {
        assert_eq!(AiProvider::Anthropic.label(), "Claude");
        assert_eq!(AiProvider::OpenAi.api_env(), OPENAI_API_ENV);
        assert_eq!(next_ai_provider(AiProvider::Gemini), AiProvider::Anthropic);
    }
}
