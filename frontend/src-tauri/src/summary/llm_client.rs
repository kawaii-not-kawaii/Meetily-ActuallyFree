use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::claude_stream;

const REQUEST_TIMEOUT_DURATION: Duration = Duration::from_secs(300);

// Generic structure for OpenAI-compatible API chat messages
#[derive(Debug, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

// Generic structure for OpenAI-compatible API chat requests
#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

// Generic structure for OpenAI-compatible API chat responses
#[derive(Deserialize, Debug)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
}

#[derive(Deserialize, Debug)]
pub struct Choice {
    pub message: MessageContent,
}

#[derive(Deserialize, Debug)]
pub struct MessageContent {
    pub content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeOutputLimits {
    default_tokens: u32,
    fallback_limit: u32,
    application_limit: u32,
    models: Vec<ClaudeModelLimit>,
    fallback_models: Vec<ClaudeFallbackModel>,
}

#[derive(Deserialize)]
pub struct ClaudeFallbackModel {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct ClaudeModelLimit {
    prefix: String,
    limit: u32,
}

// One versioned catalog serves the settings UI and request validation. Limits
// are conservative application budgets, not the model's advertised maximum.
static CLAUDE_OUTPUT_LIMITS: std::sync::LazyLock<ClaudeOutputLimits> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../src/lib/claude-output-limits.json"))
        .expect("bundled Claude output limits must be valid")
});

/// Suggested Claude models used when the live model list is unavailable. The
/// same JSON feeds the TypeScript settings UI.
pub fn claude_fallback_models() -> &'static [ClaudeFallbackModel] {
    &CLAUDE_OUTPUT_LIMITS.fallback_models
}

/// Catalog output limit before the application clamp. The longest matching
/// prefix wins, so `claude-opus-4-5` is not captured by `claude-opus-4`.
pub fn claude_catalog_limit(model_name: &str) -> u32 {
    let limits = &*CLAUDE_OUTPUT_LIMITS;
    let model = model_name.to_lowercase();
    limits.models.iter().filter(|entry| {
        model == entry.prefix || model.starts_with(&format!("{}{}", entry.prefix,
            if entry.prefix.ends_with('-') { "" } else { "-" }))
    }).max_by_key(|entry| entry.prefix.len()).map(|entry| entry.limit).unwrap_or(limits.fallback_limit)
}

pub fn claude_max_tokens(model_name: &str, configured: Option<i64>) -> Result<u32, String> {
    let limits = &*CLAUDE_OUTPUT_LIMITS;
    let maximum = limits.application_limit.min(claude_catalog_limit(model_name));
    match configured {
        None => Ok(limits.default_tokens.min(maximum)),
        Some(value) if value > 0 && value <= i64::from(maximum) => Ok(value as u32),
        Some(_) => Err(format!("Maximum Claude summary length must be between 1 and {maximum} tokens for {model_name}, or left empty.")),
    }
}

// Claude-specific request structure
#[derive(Debug, Serialize)]
pub struct ClaudeRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: String,
    pub messages: Vec<ChatMessage>,
}

/// LLM Provider enumeration for multi-provider support
#[derive(Debug, Clone, PartialEq)]
pub enum LLMProvider {
    OpenAI,
    Claude,
    Groq,
    Ollama,
    OpenRouter,
    BuiltInAI,
    ClaudeCli,
    CustomOpenAI,
}

impl LLMProvider {
    /// Parse provider from string (case-insensitive)
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAI),
            "claude" => Ok(Self::Claude),
            "claude-cli" | "claude-code" => Ok(Self::ClaudeCli),
            "groq" => Ok(Self::Groq),
            "ollama" => Ok(Self::Ollama),
            "openrouter" => Ok(Self::OpenRouter),
            "builtin-ai" | "local-llama" | "localllama" => Ok(Self::BuiltInAI),
            "custom-openai" => Ok(Self::CustomOpenAI),
            _ => Err(format!("Unsupported LLM provider: {}", s)),
        }
    }
}

/// Generates a summary using the specified LLM provider
///
/// # Arguments
/// * `client` - Reqwest HTTP client (reused for performance)
/// * `provider` - The LLM provider to use
/// * `model_name` - The specific model to use (e.g., "gpt-4", "claude-3-opus")
/// * `api_key` - API key for the provider (not needed for Ollama)
/// * `system_prompt` - System instructions for the LLM
/// * `user_prompt` - User query/content to process
/// * `ollama_endpoint` - Optional custom Ollama endpoint (defaults to localhost:11434)
/// * `custom_openai_endpoint` - Optional custom OpenAI-compatible endpoint
/// * `max_tokens` - Optional output-token cap (Claude and CustomOpenAI; other
///   OpenAI-compatible providers keep their own defaults)
/// * `temperature` - Optional temperature (for CustomOpenAI provider)
/// * `top_p` - Optional top_p (for CustomOpenAI provider)
/// * `app_data_dir` - Optional app data directory (for BuiltInAI provider)
/// * `claude_cli_path` - Optional explicit `claude` executable path (for ClaudeCli provider)
/// * `cancellation_token` - Optional token to cancel the request
///
/// # Returns
/// The generated summary text or an error message
pub async fn generate_summary(
    client: &Client,
    provider: &LLMProvider,
    model_name: &str,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    ollama_endpoint: Option<&str>,
    custom_openai_endpoint: Option<&str>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    app_data_dir: Option<&PathBuf>,
    claude_cli_path: Option<&str>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<String, String> {
    // Check if cancelled before starting
    if let Some(token) = cancellation_token {
        if token.is_cancelled() {
            return Err("Summary generation was cancelled".to_string());
        }
    }

    // Handle ClaudeCli separately (shells out to the user's `claude` CLI, no HTTP API)
    if provider == &LLMProvider::ClaudeCli {
        return crate::claude_cli::generate(
            claude_cli_path,
            model_name,
            system_prompt,
            user_prompt,
            cancellation_token,
        )
        .await;
    }

    // Handle BuiltInAI provider separately (uses local sidecar, no HTTP API)
    if provider == &LLMProvider::BuiltInAI {
        let app_data_dir = app_data_dir
            .ok_or_else(|| "app_data_dir is required for BuiltInAI provider".to_string())?;

        return crate::summary::summary_engine::generate_with_builtin(
            app_data_dir,
            model_name,
            system_prompt,
            user_prompt,
            cancellation_token,
        )
        .await
        .map_err(|e| e.to_string());
    }

    // Claude is streamed over SSE with an idle (not total) timeout; see claude_stream.
    if provider == &LLMProvider::Claude {
        return generate_claude(
            client,
            CLAUDE_MESSAGES_URL,
            model_name,
            api_key,
            system_prompt,
            user_prompt,
            max_tokens,
            cancellation_token,
        )
        .await;
    }

    let (api_url, mut headers) = match provider {
        LLMProvider::OpenAI => (
            "https://api.openai.com/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::Groq => (
            "https://api.groq.com/openai/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::OpenRouter => (
            "https://openrouter.ai/api/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::Ollama => {
            let host = ollama_endpoint
                .map(|s| s.to_string())
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            (
                format!("{}/v1/chat/completions", host),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::CustomOpenAI => {
            let endpoint = custom_openai_endpoint
                .ok_or_else(|| "Custom OpenAI endpoint not configured".to_string())?;
            (
                format!("{}/chat/completions", endpoint.trim_end_matches('/')),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::Claude => unreachable!("Claude is handled before this match statement"),
        LLMProvider::BuiltInAI => {
            // This case is handled earlier with early returns
            unreachable!("BuiltInAI is handled before this match statement")
        }
        LLMProvider::ClaudeCli => {
            // This case is handled earlier with early returns
            unreachable!("ClaudeCli is handled before this match statement")
        }
    };

    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", api_key)
            .parse()
            .map_err(|_| "Invalid authorization header".to_string())?,
    );
    headers.insert(
        header::CONTENT_TYPE,
        "application/json"
            .parse()
            .map_err(|_| "Invalid content type".to_string())?,
    );

    // Build request body based on provider
    // For CustomOpenAI, apply optional parameters if provided
    let (max_tokens_val, temperature_val, top_p_val) = if provider == &LLMProvider::CustomOpenAI {
        (max_tokens, temperature, top_p)
    } else {
        (None, None, None)
    };
    let request_body = serde_json::json!(ChatRequest {
        model: model_name.to_string(),
        messages: vec![
            ChatMessage { role: "system".to_string(), content: system_prompt.to_string() },
            ChatMessage { role: "user".to_string(), content: user_prompt.to_string() },
        ],
        max_tokens: max_tokens_val,
        temperature: temperature_val,
        top_p: top_p_val,
    });

    info!("🐞 LLM Request to {}: model={}", provider_name(provider), model_name);

    // Send request with timeout and cancellation support
    let request_future = client
        .post(api_url)
        .headers(headers)
        .json(&request_body)
        .timeout(REQUEST_TIMEOUT_DURATION)
        .send();

    // Use tokio::select to race between cancellation and request completion
    let response = if let Some(token) = cancellation_token {
        tokio::select! {
            result = request_future => {
                result.map_err(|e| {
                    if e.is_timeout() {
                        format!("LLM request timed out after {} seconds", REQUEST_TIMEOUT_DURATION.as_secs())
                    } else {
                        format!("Failed to send request to LLM: {}", e)
                    }
                })?
            }
            _ = token.cancelled() => {
                return Err("Summary generation was cancelled".to_string());
            }
        }
    } else {
        request_future.await.map_err(|e| {
            if e.is_timeout() {
                format!("LLM request timed out after {} seconds", REQUEST_TIMEOUT_DURATION.as_secs())
            } else {
                format!("Failed to send request to LLM: {}", e)
            }
        })?
    };

    if !response.status().is_success() {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("LLM API request failed: {}", error_body));
    }

    let chat_response = response
        .json::<ChatResponse>()
        .await
        .map_err(|e| format!("Failed to parse LLM response: {}", e))?;

    info!("🐞 LLM Response received from {}", provider_name(provider));

    let content = chat_response
        .choices
        .get(0)
        .ok_or("No content in LLM response")?
        .message
        .content
        .trim();
    Ok(content.to_string())
}

const CLAUDE_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

/// One streamed Claude request, retried on transient failures. `url` is a
/// parameter so tests can aim it at a local fake SSE server.
pub(crate) async fn generate_claude(
    client: &Client,
    url: &str,
    model_name: &str,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: Option<u32>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<String, String> {
    let mut headers = header::HeaderMap::new();
    headers.insert("x-api-key", api_key.parse().map_err(|_| "Invalid API key format".to_string())?);
    headers.insert("anthropic-version", header::HeaderValue::from_static("2023-06-01"));
    headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"));

    let body = serde_json::json!(ClaudeRequest {
        system: system_prompt.to_string(),
        model: model_name.to_string(),
        max_tokens: claude_max_tokens(model_name, max_tokens.map(i64::from))?,
        messages: vec![ChatMessage { role: "user".to_string(), content: user_prompt.to_string() }],
    });

    info!("🐞 LLM Request to Claude: model={}", model_name);
    let result = claude_stream::stream_message_with_retry(
        client, url, headers, body, claude_stream::CLAUDE_IDLE_TIMEOUT, cancellation_token,
        claude_stream::RetryPolicy::default(),
    )
    .await
    .map_err(|e| e.to_string())?;
    info!("🐞 LLM Response received from Claude");
    claude_stream::complete_text(result)
}

/// Helper function to get provider name for logging
fn provider_name(provider: &LLMProvider) -> &str {
    match provider {
        LLMProvider::OpenAI => "OpenAI",
        LLMProvider::Claude => "Claude",
        LLMProvider::ClaudeCli => "Claude Code CLI",
        LLMProvider::Groq => "Groq",
        LLMProvider::Ollama => "Ollama",
        LLMProvider::BuiltInAI => "Built-in AI",
        LLMProvider::OpenRouter => "OpenRouter",
        LLMProvider::CustomOpenAI => "Custom OpenAI",
    }
}

#[cfg(test)]
mod claude_output_tests {
    use super::*;

    #[test]
    fn catalog_resolves_every_model_to_its_limit_by_longest_prefix() {
        let cases: &[(&str, u32)] = &[
            ("claude-3-opus-20240229", 4096), ("claude-3-5-sonnet-latest", 8192), ("claude-3-7-sonnet-latest", 64000),
            ("claude-sonnet-4-20250514", 64000), ("claude-sonnet-4-5-20250929", 64000), ("claude-sonnet-4-6", 128000),
            ("claude-sonnet-5", 128000), ("claude-sonnet-5-5", 128000),
            ("claude-haiku-4-5-20251001", 64000), ("claude-haiku-5", 128000), ("claude-haiku-5-5", 128000),
            ("claude-opus-4-1-20250805", 32000), ("claude-opus-4-20250514", 32000), ("claude-opus-4-5-20251101", 64000),
            ("claude-opus-4-6", 128000), ("claude-opus-4-7", 128000), ("claude-opus-4-8", 128000),
            ("claude-opus-5", 128000), ("claude-opus-5-5", 128000),
            ("claude-fable-5-2", 128000), ("claude-mythos-5-2", 128000), ("claude-unknown", 8192),
        ];
        for (model, limit) in cases {
            assert_eq!(claude_catalog_limit(model), *limit, "{model}");
        }
    }

    #[test]
    fn fallback_models_have_no_retired_entries() {
        let ids: Vec<&str> = claude_fallback_models().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5", "claude-haiku-4-5"]);
    }

    #[test]
    fn validates_budget_without_confusing_model_generations() {
        assert_eq!(claude_max_tokens("claude-3-opus", None).unwrap(), 4096);
        assert_eq!(claude_max_tokens("claude-3-5-sonnet-latest", None).unwrap(), 8192);
        assert_eq!(claude_max_tokens("claude-sonnet-4-5", None).unwrap(), 8192);
        assert_eq!(claude_max_tokens("claude-sonnet-4-5", Some(32000)).unwrap(), 32000);
        assert_eq!(claude_max_tokens("claude-sonnet-4-50-unknown", None).unwrap(), 8192);
        assert!(claude_max_tokens("claude-3-opus", Some(8192)).is_err());
        for value in [0, -1, 64001, i64::MAX] {
            assert!(claude_max_tokens("claude-sonnet-4-5", Some(value)).is_err());
        }
        assert!(claude_max_tokens("claude-unknown", Some(9000)).is_err());
    }

    #[tokio::test]
    async fn summary_output_setting_round_trips_and_clears() {
        use crate::database::repositories::setting::SettingsRepository;
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        SettingsRepository::save_model_config(&pool, "claude", "claude-sonnet-4-5", "large-v3", None, Some(32000)).await.unwrap();
        assert_eq!(SettingsRepository::get_summary_max_tokens(&pool).await.unwrap(), Some(32000));
        SettingsRepository::save_model_config(&pool, "claude", "claude-sonnet-4-5", "large-v3", None, None).await.unwrap();
        assert_eq!(SettingsRepository::get_summary_max_tokens(&pool).await.unwrap(), None);
        assert!(!include_bytes!("../../migrations/20260914010000_add_summary_max_tokens.sql").contains(&b'\r'));
    }
}
