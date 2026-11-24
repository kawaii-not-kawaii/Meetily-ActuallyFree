//! Streamed (SSE) Claude Messages client.
//!
//! Large summaries can take many minutes, so there is deliberately no total
//! request timeout: a connect timeout bounds the TCP/TLS handshake and an idle
//! timeout bounds the silence between received bytes. Anthropic sends periodic
//! `ping` events, so any bytes (including pings) reset the idle clock.
//! Cancellation drops the response, which closes the connection.

use futures_util::StreamExt;
use reqwest::{header::HeaderMap, Client};
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const CLAUDE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub const CLAUDE_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// One process-wide client so connections are reused across summary,
/// translation and live-assistant calls. It carries no total timeout; callers
/// that want one (non-streaming providers) set it per request.
pub fn shared_http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(CLAUDE_CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClaudeUsage {
    pub input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    /// Cumulative: the last `message_delta` value wins.
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClaudeStreamResult {
    pub text: String,
    pub stop_reason: Option<String>,
    /// `stop_details` from a `refusal` (category/explanation), kept raw.
    pub stop_details: Option<Value>,
    pub usage: ClaudeUsage,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeStreamError {
    Cancelled,
    IdleTimeout(Duration),
    Connect(String),
    Http { status: u16, body: String },
    /// An `error` event received mid-stream.
    Stream { error_type: String, message: String },
    Malformed(String),
}

impl std::fmt::Display for ClaudeStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "Summary generation was cancelled"),
            Self::IdleTimeout(d) => write!(
                f,
                "Claude stopped responding: no data received for {} seconds (idle timeout)",
                d.as_secs()
            ),
            Self::Connect(e) => write!(f, "Failed to send request to LLM: {e}"),
            Self::Http { body, .. } => write!(f, "LLM API request failed: {body}"),
            Self::Stream { error_type, message } => {
                write!(f, "Claude stream error ({error_type}): {message}")
            }
            Self::Malformed(e) => write!(f, "Failed to parse Claude stream: {e}"),
        }
    }
}

/// Incremental SSE parser: feed raw bytes, get complete `data:` payloads.
#[derive(Default)]
struct SseParser {
    buf: Vec<u8>,
    data: Vec<String>,
}

impl SseParser {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut events = Vec::new();
        // Split on bytes, not chars: a UTF-8 sequence may straddle chunks.
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
            // `event:`, `id:` and comment lines are not needed: the JSON
            // payload carries its own `type`.
        }
        events
    }
}

fn apply_usage(usage: &mut ClaudeUsage, v: &Value) {
    let get = |k: &str| v.get(k).and_then(Value::as_u64);
    if let Some(n) = get("input_tokens") { usage.input_tokens = n; }
    if let Some(n) = get("cache_read_input_tokens") { usage.cache_read_input_tokens = n; }
    if let Some(n) = get("cache_creation_input_tokens") { usage.cache_creation_input_tokens = n; }
    if let Some(n) = get("output_tokens") { usage.output_tokens = n; }
}

/// Folds one event payload into the result. Returns true at `message_stop`.
fn apply_event(result: &mut ClaudeStreamResult, payload: &str) -> Result<bool, ClaudeStreamError> {
    let v: Value = serde_json::from_str(payload)
        .map_err(|e| ClaudeStreamError::Malformed(e.to_string()))?;
    match v.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            if let Some(u) = v.pointer("/message/usage") { apply_usage(&mut result.usage, u); }
        }
        Some("content_block_delta") => {
            if v.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta") {
                if let Some(t) = v.pointer("/delta/text").and_then(Value::as_str) {
                    result.text.push_str(t);
                }
            }
        }
        Some("message_delta") => {
            if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                result.stop_reason = Some(r.to_string());
            }
            if let Some(d) = v.pointer("/delta/stop_details").filter(|d| !d.is_null()) {
                result.stop_details = Some(d.clone());
            }
            if let Some(u) = v.get("usage") { apply_usage(&mut result.usage, u); }
        }
        Some("error") => {
            let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("unknown").to_string();
            return Err(ClaudeStreamError::Stream {
                error_type: s("/error/type"),
                message: s("/error/message"),
            });
        }
        Some("message_stop") => return Ok(true),
        _ => {} // ping, content_block_start/stop, unknown future events
    }
    Ok(false)
}

/// Sends `body` (with `stream: true` forced) and accumulates the response.
pub async fn stream_message(
    client: &Client,
    url: &str,
    headers: HeaderMap,
    mut body: Value,
    idle_timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<ClaudeStreamResult, ClaudeStreamError> {
    body["stream"] = Value::Bool(true);
    let never = CancellationToken::new();
    let cancel = cancel.unwrap_or(&never);

    let send = client.post(url).headers(headers).json(&body).send();
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(ClaudeStreamError::Cancelled),
        r = tokio::time::timeout(idle_timeout, send) => match r {
            Err(_) => return Err(ClaudeStreamError::IdleTimeout(idle_timeout)),
            Ok(Err(e)) => return Err(ClaudeStreamError::Connect(e.to_string())),
            Ok(Ok(r)) => r,
        },
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_else(|_| "Unknown error".into());
        return Err(ClaudeStreamError::Http { status, body });
    }

    let mut stream = response.bytes_stream();
    let mut parser = SseParser::default();
    let mut result = ClaudeStreamResult::default();
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ClaudeStreamError::Cancelled),
            n = tokio::time::timeout(idle_timeout, stream.next()) => n,
        };
        let chunk = match next {
            Err(_) => return Err(ClaudeStreamError::IdleTimeout(idle_timeout)),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(ClaudeStreamError::Connect(e.to_string())),
            Ok(Some(Ok(c))) => c,
        };
        for payload in parser.push(&chunk) {
            if apply_event(&mut result, &payload)? {
                return Ok(result);
            }
        }
    }
    Ok(result)
}

/// Stop-reason policy for a finished stream. `max_tokens`, `refusal` and
/// anything unknown still fail (retry/partial-save arrive in later tickets).
pub fn complete_text(result: ClaudeStreamResult) -> Result<String, String> {
    match result.stop_reason.as_deref() {
        Some("end_turn" | "stop_sequence") => {}
        Some("max_tokens") => return Err("Claude reached the output limit before completing the summary. Increase Maximum summary length in Model Settings or request a shorter summary, then retry. The incomplete result was not saved.".into()),
        Some("refusal") => {
            let detail = result.stop_details.as_ref().map(|d| {
                let g = |k: &str| d.get(k).and_then(Value::as_str);
                match (g("category"), g("explanation")) {
                    (Some(c), Some(e)) => format!(" ({c}: {e})"),
                    (Some(c), None) => format!(" ({c})"),
                    (None, Some(e)) => format!(" ({e})"),
                    _ => String::new(),
                }
            }).unwrap_or_default();
            return Err(format!("Claude declined the request (stop reason: refusal){detail}."));
        }
        Some(reason) => return Err(format!("Claude did not complete the response (stop reason: {reason}). Please retry.")),
        None => return Err("Claude response did not include a completion reason; incomplete output was not accepted.".into()),
    }
    let text = result.text.trim();
    if text.is_empty() { return Err("No text in Claude response".into()); }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests;
