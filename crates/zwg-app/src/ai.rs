pub const ANTHROPIC_API_ENV: &str = "ANTHROPIC_API_KEY";
pub const OPENAI_API_ENV: &str = "OPENAI_API_KEY";
pub const GEMINI_API_ENV: &str = "GEMINI_API_KEY";
pub const ANTHROPIC_MODEL_ENV: &str = "ZWG_CLAUDE_MODEL";
pub const OPENAI_MODEL_ENV: &str = "ZWG_OPENAI_MODEL";
pub const GEMINI_MODEL_ENV: &str = "ZWG_GEMINI_MODEL";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";
const DEFAULT_OPENAI_MODEL: &str = "gpt-5-mini";
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn provider_specific_defaults_are_stable() {
        assert_eq!(AiProvider::Anthropic.label(), "Claude");
        assert_eq!(AiProvider::OpenAi.api_env(), OPENAI_API_ENV);
        assert_eq!(next_ai_provider(AiProvider::Gemini), AiProvider::Anthropic);
    }
}
