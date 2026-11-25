use crate::summary::claude_limits::{
    claude_fallback_models, effective_max_tokens, resolve_claude_limit, validate_cap, ClaudeLimit,
    LiveLimits,
};
use crate::summary::claude_stream::shared_http_client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};
use tauri::command;

/// Anthropic (Claude) model information returned to frontend
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnthropicModel {
    pub id: String,
    pub display_name: Option<String>,
    /// Output cap; absent on bundled fallback entries.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Context window.
    #[serde(default)]
    pub max_input_tokens: Option<u32>,
    /// The capability tree as reported (effort, thinking types, structured
    /// outputs, ...). Kept raw: each leaf is `{"supported": bool}`.
    #[serde(default)]
    pub capabilities: Option<Value>,
}

/// One page of `GET /v1/models` (cursor scheme: `after_id` / `has_more`).
#[derive(Debug, Deserialize)]
struct ModelsPage {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    last_id: Option<String>,
}

/// Cache entry for models
struct CacheEntry {
    models: Vec<AnthropicModel>,
    fetched_at: Instant,
}

/// Global cache for Anthropic models (5 minute TTL)
static MODELS_CACHE: RwLock<Option<CacheEntry>> = RwLock::new(None);

/// Per-model limit data, separate from the list cache. Filled by both the list
/// and single-model retrieves; failures are never cached.
static MODEL_CACHE: LazyLock<RwLock<HashMap<String, (AnthropicModel, Instant)>>> =
    LazyLock::new(Default::default);

/// Cache TTL in seconds
const CACHE_TTL_SECS: u64 = 300;

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com";
const MODELS_TIMEOUT: Duration = Duration::from_secs(5);
const PAGE_SIZE: u32 = 1000;
/// Bounds a server that keeps reporting more pages.
const MAX_PAGES: usize = 20;

fn cache_model(requested_id: &str, model: AnthropicModel) {
    if let Ok(mut cache) = MODEL_CACHE.write() {
        cache.insert(requested_id.to_string(), (model, Instant::now()));
    }
}

fn cached_model(id: &str) -> Option<AnthropicModel> {
    let cache = MODEL_CACHE.read().ok()?;
    let (model, fetched_at) = cache.get(id)?;
    (fetched_at.elapsed() < Duration::from_secs(CACHE_TTL_SECS)).then(|| model.clone())
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: reqwest::Url,
    api_key: &str,
) -> Result<T, String> {
    let response = client
        .get(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .timeout(MODELS_TIMEOUT)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Anthropic API returned status {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

fn models_url(base: &str) -> Result<reqwest::Url, String> {
    reqwest::Url::parse(&format!("{base}/v1/models")).map_err(|e| e.to_string())
}

/// Every Claude model the key can see, following all pages. A partial list is
/// an error rather than a short result.
async fn fetch_models(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
) -> Result<Vec<AnthropicModel>, String> {
    let mut models = Vec::new();
    let mut after_id: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = models_url(base)?;
        url.query_pairs_mut().append_pair("limit", &PAGE_SIZE.to_string());
        if let Some(cursor) = &after_id {
            url.query_pairs_mut().append_pair("after_id", cursor);
        }
        let page: ModelsPage = get_json(client, url, api_key).await?;
        models.extend(page.data.into_iter().filter(|m| is_chat_model(&m.id)));
        if !page.has_more {
            for model in &models {
                cache_model(&model.id, model.clone());
            }
            return Ok(models);
        }
        if page.last_id.is_none() || page.last_id == after_id {
            return Err("Anthropic model list pagination did not advance".into());
        }
        after_id = page.last_id;
    }
    Err(format!("Anthropic model list exceeded {MAX_PAGES} pages"))
}

/// One model's live data: cached, else `GET /v1/models/{id}`.
async fn live_model(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    id: &str,
) -> Result<AnthropicModel, String> {
    if let Some(model) = cached_model(id) {
        return Ok(model);
    }
    let mut url = models_url(base)?;
    url.path_segments_mut()
        .map_err(|_| "Invalid Anthropic base URL".to_string())?
        .push(id);
    let model: AnthropicModel = get_json(client, url, api_key).await?;
    cache_model(id, model.clone());
    Ok(model)
}

/// The limits for `model`: live data when the Models API supplies it, the
/// bundled catalog when there is no key, the call fails or the model is unlisted.
async fn model_limit(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
    model: &str,
) -> ClaudeLimit {
    let live = match api_key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => match live_model(client, base, key, model).await {
            Ok(m) => Some(LiveLimits {
                max_tokens: m.max_tokens,
                max_input_tokens: m.max_input_tokens,
            }),
            Err(e) => {
                log::warn!("No live limits for Claude model {model}: {e}. Using bundled catalog.");
                None
            }
        },
        None => None,
    };
    resolve_claude_limit(model, live)
}

async fn check_summary_cap(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
    model: &str,
    cap: Option<i64>,
) -> Result<(), String> {
    if cap.is_none() {
        return Ok(());
    }
    validate_cap(model, &model_limit(client, base, api_key, model).await, cap)
}

async fn summary_max_tokens(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
    model: &str,
    cap: Option<i64>,
) -> u32 {
    effective_max_tokens(&model_limit(client, base, api_key, model).await, cap)
}

/// Save-time validation of the user's summary cap for a Claude model.
pub async fn validate_claude_summary_cap(
    api_key: Option<&str>,
    model: &str,
    cap: Option<i64>,
) -> Result<(), String> {
    check_summary_cap(shared_http_client(), ANTHROPIC_API_BASE, api_key, model, cap).await
}

/// Output length to request for a summary run: the model maximum, or the
/// user's cap clamped to it.
pub async fn claude_summary_max_tokens(
    api_key: &str,
    model: &str,
    cap: Option<i64>,
) -> u32 {
    summary_max_tokens(shared_http_client(), ANTHROPIC_API_BASE, Some(api_key), model, cap).await
}

/// Limits the settings UI shows for the selected model.
#[command]
pub async fn get_claude_model_limit(
    model: String,
    api_key: Option<String>,
) -> Result<ClaudeLimit, String> {
    Ok(model_limit(shared_http_client(), ANTHROPIC_API_BASE, api_key.as_deref(), &model).await)
}

/// Fallback models when the API fetch fails. The list lives once in the shared
/// catalog JSON, which the TypeScript settings UI also reads.
fn get_fallback_models() -> Vec<AnthropicModel> {
    claude_fallback_models()
        .iter()
        .map(|m| AnthropicModel {
            id: m.id.clone(),
            display_name: Some(m.name.clone()),
            max_tokens: None,
            max_input_tokens: None,
            capabilities: None,
        })
        .collect()
}

/// Check if model is a chat-capable model
fn is_chat_model(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    // Include Claude models only
    id.starts_with("claude-")
}

/// Fetch Anthropic models from API
///
/// # Arguments
/// * `api_key` - Anthropic API key
///
/// # Returns
/// Vector of available models, or fallback models on error
#[command]
pub async fn get_anthropic_models(api_key: Option<String>) -> Result<Vec<AnthropicModel>, String> {
    // Return fallback if no API key provided
    let api_key = match api_key {
        Some(key) if !key.trim().is_empty() => key.trim().to_string(),
        _ => {
            log::info!("No Anthropic API key provided, returning fallback models");
            return Ok(get_fallback_models());
        }
    };

    // Check cache first
    {
        let cache = MODELS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(entry) = cache.as_ref() {
            if entry.fetched_at.elapsed() < Duration::from_secs(CACHE_TTL_SECS) {
                log::info!(
                    "Returning cached Anthropic models ({} models)",
                    entry.models.len()
                );
                return Ok(entry.models.clone());
            }
        }
    }

    // Fetch from API
    log::info!("Fetching Anthropic models from API...");
    let models = match fetch_models(shared_http_client(), ANTHROPIC_API_BASE, &api_key).await {
        Ok(models) => models,
        Err(e) => {
            log::warn!("Failed to fetch Anthropic models: {}. Using fallback.", e);
            return Ok(get_fallback_models());
        }
    };

    // If no models returned, use fallback
    if models.is_empty() {
        log::warn!("No chat models returned from Anthropic API. Using fallback.");
        return Ok(get_fallback_models());
    }

    log::info!("Fetched {} Anthropic models from API", models.len());

    // Update cache
    {
        let mut cache = MODELS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(CacheEntry {
            models: models.clone(),
            fetched_at: Instant::now(),
        });
    }

    Ok(models)
}

/// Clear the models cache (useful when API key changes)
pub fn clear_cache() {
    if let Ok(mut cache) = MODELS_CACHE.write() {
        *cache = None;
        log::info!("Anthropic models cache cleared");
    }
    if let Ok(mut cache) = MODEL_CACHE.write() {
        cache.clear();
    }
}

#[cfg(test)]
mod tests;
