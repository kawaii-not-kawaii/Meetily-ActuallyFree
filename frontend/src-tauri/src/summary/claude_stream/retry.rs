//! Retries for transient Claude failures, wrapping the whole streamed request.
//!
//! A retry always restarts the request from scratch: text accumulated by a
//! failed attempt is dropped with it and is never returned.
//!
//! Progress reaches the caller through a task-local observer
//! ([`observe_retries`]), so the provider-neutral summary call chain needs no
//! extra parameter. Task-locals do not cross `tokio::spawn`: futures that should
//! report retries must be polled on the observing task.

use super::{stream_attempt, ClaudeStreamError, ClaudeStreamResult};
use reqwest::{header::HeaderMap, Client};
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// Exponential step before the first retry; doubles for each one after.
    pub base_delay: Duration,
    /// Upper bound on any single wait, including one requested by `retry-after`.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// Wait before retry number `retry` (1-based). `jitter` in `0.0..=1.0`
    /// places the backoff between half and all of the exponential step;
    /// `retry_after` is a floor under it.
    fn delay(&self, retry: u32, retry_after: Option<Duration>, jitter: f64) -> Duration {
        let doublings = retry.saturating_sub(1).min(16);
        let step = self.base_delay.saturating_mul(1 << doublings).min(self.max_delay);
        let backoff = step / 2 + (step / 2).mul_f64(jitter.clamp(0.0, 1.0));
        backoff.max(retry_after.unwrap_or_default()).min(self.max_delay)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryEvent {
    /// A transient failure; retry `retry` of `max_retries` starts after `delay`.
    Retrying { retry: u32, max_retries: u32, delay: Duration },
    /// A retried request was accepted and is streaming.
    Resumed,
}

impl RetryEvent {
    /// The `summary-progress` stage and message shown to the user.
    pub fn progress(&self) -> (&'static str, String) {
        match self {
            Self::Retrying { retry, max_retries, .. } => (
                "retrying",
                format!("Claude is temporarily unavailable, retrying ({retry} of {max_retries})…"),
            ),
            Self::Resumed => ("generating", "Generating summary…".to_string()),
        }
    }
}

pub type RetryObserver = Arc<dyn Fn(RetryEvent) + Send + Sync>;

tokio::task_local! {
    static OBSERVER: RetryObserver;
}

/// Runs `work`, reporting the retries of every Claude request inside it.
pub async fn observe_retries<F: Future>(observer: RetryObserver, work: F) -> F::Output {
    OBSERVER.scope(observer, work).await
}

fn notify(event: RetryEvent) {
    // Callers without an observer (e.g. the live assistant) retry silently.
    let _ = OBSERVER.try_with(|observer| observer(event));
}

/// Error types of a mid-stream `error` event that a new request can clear.
const TRANSIENT_STREAM_ERRORS: [&str; 4] =
    ["overloaded_error", "api_error", "rate_limit_error", "timeout_error"];

/// A spend cap does not clear by waiting. The API marks it with this code and
/// sends no `retry-after`.
fn is_spend_limit(body: &str) -> bool {
    serde_json::from_str::<Value>(body).is_ok_and(|v| {
        v.pointer("/error/details/error_code").and_then(Value::as_str)
            == Some("enforced_spend_limit_reached")
    })
}

fn is_transient(error: &ClaudeStreamError) -> bool {
    match error {
        ClaudeStreamError::Http { status: 429, body, retry_after } => {
            retry_after.is_some() || !is_spend_limit(body)
        }
        ClaudeStreamError::Http { status, .. } => {
            matches!(status, 408 | 409 | 500 | 502 | 503 | 504 | 529)
        }
        ClaudeStreamError::Stream { error_type, .. } => {
            TRANSIENT_STREAM_ERRORS.contains(&error_type.as_str())
        }
        ClaudeStreamError::Connect(_) | ClaudeStreamError::IdleTimeout(_) => true,
        ClaudeStreamError::Cancelled | ClaudeStreamError::Malformed(_) => false,
    }
}

/// Failure category for logs. Never includes response bodies or messages, which
/// could echo request content.
fn log_kind(error: &ClaudeStreamError) -> String {
    match error {
        ClaudeStreamError::Http { status, .. } => format!("HTTP {status}"),
        ClaudeStreamError::Stream { error_type, .. } => format!("stream {error_type}"),
        ClaudeStreamError::Connect(_) => "connection failure".into(),
        ClaudeStreamError::IdleTimeout(_) => "idle timeout".into(),
        ClaudeStreamError::Cancelled => "cancelled".into(),
        ClaudeStreamError::Malformed(_) => "malformed stream".into(),
    }
}

/// [`super::stream_message`] under `policy`. Cancellation aborts a pending
/// backoff as well as an in-flight request.
#[allow(clippy::too_many_arguments)]
pub async fn stream_message_with_retry(
    client: &Client,
    url: &str,
    headers: HeaderMap,
    body: Value,
    idle_timeout: Duration,
    cancel: Option<&CancellationToken>,
    policy: RetryPolicy,
) -> Result<ClaudeStreamResult, ClaudeStreamError> {
    let never = CancellationToken::new();
    let cancel = cancel.unwrap_or(&never);
    let mut retry = 0;
    loop {
        let error = match stream_attempt(
            client, url, headers.clone(), body.clone(), idle_timeout, cancel,
            || {
                if retry > 0 {
                    notify(RetryEvent::Resumed);
                }
            },
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(error) => error,
        };
        if retry >= policy.max_retries || !is_transient(&error) {
            return Err(error);
        }
        retry += 1;
        let retry_after = match &error {
            ClaudeStreamError::Http { retry_after, .. } => *retry_after,
            _ => None,
        };
        let delay = policy.delay(retry, retry_after, rand::random());
        tracing::warn!(
            "Claude request failed ({}); retry {} of {} in {:?}",
            log_kind(&error), retry, policy.max_retries, delay
        );
        notify(RetryEvent::Retrying { retry, max_retries: policy.max_retries, delay });
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ClaudeStreamError::Cancelled),
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

#[cfg(test)]
mod tests;
