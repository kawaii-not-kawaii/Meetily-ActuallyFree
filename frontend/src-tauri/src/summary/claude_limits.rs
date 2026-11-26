//! Claude output/input limits: live Models API data first, bundled catalog second.
//!
//! The effective summary length is the model's maximum output unless the user
//! set a lower cap (`settings.summaryMaxTokens`). Everything here is pure;
//! `anthropic::anthropic` owns the network and caching.

use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Catalog {
    fallback_limit: u32,
    models: Vec<CatalogEntry>,
    fallback_models: Vec<ClaudeFallbackModel>,
}

#[derive(Deserialize)]
struct CatalogEntry {
    prefix: String,
    limit: u32,
}

#[derive(Deserialize)]
pub struct ClaudeFallbackModel {
    pub id: String,
    pub name: String,
}

// The TypeScript settings UI reads the fallback model list from the same JSON.
static CATALOG: std::sync::LazyLock<Catalog> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../src/lib/claude-output-limits.json"))
        .expect("bundled Claude output limits must be valid")
});

/// Suggested Claude models used when the live model list is unavailable.
pub fn claude_fallback_models() -> &'static [ClaudeFallbackModel] {
    &CATALOG.fallback_models
}

/// Catalog output limit. The longest matching prefix wins, so
/// `claude-opus-4-5` is not captured by `claude-opus-4`.
fn catalog_limit(model: &str) -> Option<u32> {
    let model = model.to_lowercase();
    CATALOG.models.iter().filter(|entry| {
        model == entry.prefix || model.starts_with(&format!("{}{}", entry.prefix,
            if entry.prefix.ends_with('-') { "" } else { "-" }))
    }).max_by_key(|entry| entry.prefix.len()).map(|entry| entry.limit)
}

/// The limit fields the Models API reported for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveLimits {
    pub max_tokens: Option<u32>,
    pub max_input_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LimitSource {
    Live,
    Catalog,
    /// Neither source knows the model; the conservative fallback applies.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeLimit {
    pub max_output_tokens: u32,
    /// Context window. Only the live API supplies it.
    pub max_input_tokens: Option<u32>,
    pub source: LimitSource,
}

/// `live` is `None` when the Models call failed or did not list the model.
pub fn resolve_claude_limit(model: &str, live: Option<LiveLimits>) -> ClaudeLimit {
    let max_input_tokens = live.and_then(|l| l.max_input_tokens).filter(|n| *n > 0);
    let (max_output_tokens, source) =
        match (live.and_then(|l| l.max_tokens).filter(|n| *n > 0), catalog_limit(model)) {
            (Some(max), _) => (max, LimitSource::Live),
            (None, Some(max)) => (max, LimitSource::Catalog),
            (None, None) => (CATALOG.fallback_limit, LimitSource::Unknown),
        };
    ClaudeLimit { max_output_tokens, max_input_tokens, source }
}

/// Save-time check of the user's cap. `None` means "use the model maximum".
pub fn validate_cap(model: &str, limit: &ClaudeLimit, cap: Option<i64>) -> Result<(), String> {
    let maximum = limit.max_output_tokens;
    match cap {
        Some(value) if value < 1 || value > i64::from(maximum) => {
            let unknown = if limit.source == LimitSource::Unknown {
                " (this model's real limit is unknown)"
            } else {
                ""
            };
            Err(format!("Maximum Claude summary length must be between 1 and {maximum} tokens for {model}{unknown}, or left empty."))
        }
        _ => Ok(()),
    }
}

/// Use-time output length. The cap is re-clamped because the model (and so
/// its limit) may have changed since the cap was saved.
pub fn effective_max_tokens(limit: &ClaudeLimit, cap: Option<i64>) -> u32 {
    match cap {
        Some(value) if value > 0 => {
            u32::try_from(value).map_or(limit.max_output_tokens, |v| v.min(limit.max_output_tokens))
        }
        _ => limit.max_output_tokens,
    }
}

#[cfg(test)]
mod tests;
