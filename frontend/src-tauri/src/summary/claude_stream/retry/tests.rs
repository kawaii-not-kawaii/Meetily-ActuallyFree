//! Retry policy against the fake SSE server: attempt counts and waits are
//! observed from outside (requests received, retry events emitted).
use super::super::tests::*;
use super::super::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const FAST: RetryPolicy = RetryPolicy {
    max_retries: 3,
    base_delay: Duration::from_millis(20),
    max_delay: Duration::from_secs(60),
};

fn http(status_line: &str, extra_headers: &str, body: &str) -> Vec<Step> {
    vec![Step::Write(format!(
        "HTTP/1.1 {status_line}\r\n{extra_headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    ))]
}

fn api_error(status_line: &str, extra_headers: &str, error_type: &str) -> Vec<Step> {
    http(status_line, extra_headers, &format!(r#"{{"type":"error","error":{{"type":"{error_type}","message":"{error_type} happened"}}}}"#))
}

fn success(text: &str) -> Vec<Step> {
    let mut s = vec![ok_head(), start(), text_delta(text)];
    s.extend(finish("end_turn"));
    s
}

fn overloaded() -> Vec<Step> {
    api_error("529 Overloaded", "", "overloaded_error")
}

/// Runs the retrying request and records the retry events it emits.
async fn run(
    fake: &Fake,
    policy: RetryPolicy,
    idle_ms: u64,
    cancel: Option<&CancellationToken>,
) -> (Result<ClaudeStreamResult, ClaudeStreamError>, Vec<RetryEvent>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let result = observe_retries(
        Arc::new(move |e| sink.lock().unwrap().push(e)),
        stream_message_with_retry(
            &reqwest::Client::new(), &fake.url, reqwest::header::HeaderMap::new(),
            serde_json::json!({"model":"claude-x","max_tokens":10,"messages":[]}),
            Duration::from_millis(idle_ms), cancel, policy,
        ),
    )
    .await;
    let events = events.lock().unwrap().clone();
    (result, events)
}

fn waits(events: &[RetryEvent]) -> Vec<(u32, u32, Duration)> {
    events.iter().filter_map(|e| match e {
        RetryEvent::Retrying { retry, max_retries, delay } => Some((*retry, *max_retries, *delay)),
        RetryEvent::Resumed => None,
    }).collect()
}

fn attempts(fake: &Fake) -> usize {
    fake.requests.lock().unwrap().len()
}

/// Jitter keeps each wait within [half, full] of the exponential step.
fn assert_backoff(delay: Duration, full_ms: u64) {
    assert!(delay >= Duration::from_millis(full_ms / 2) && delay <= Duration::from_millis(full_ms), "{delay:?} vs {full_ms}ms");
}

#[tokio::test]
async fn overload_529_is_retried_then_succeeds() {
    let fake = serve_sequence(vec![overloaded(), success("ok")]).await;
    let (result, events) = run(&fake, FAST, 2000, None).await;
    assert_eq!(result.unwrap().text, "ok");
    assert_eq!(attempts(&fake), 2);
    let w = waits(&events);
    assert_eq!((w.len(), w[0].0, w[0].1), (1, 1, 3));
    assert_backoff(w[0].2, 20);
    // The UI is told the retried request got through, so the notice can clear.
    assert_eq!(events.last(), Some(&RetryEvent::Resumed));
}

#[tokio::test]
async fn first_time_success_emits_no_retry_events() {
    let fake = serve(success("ok")).await;
    let (result, events) = run(&fake, FAST, 2000, None).await;
    assert!(result.is_ok());
    assert!(events.is_empty(), "{events:?}");
}

#[tokio::test]
async fn retry_after_is_a_floor_on_the_wait() {
    let fake = serve_sequence(vec![
        api_error("429 Too Many Requests", "retry-after: 1\r\n", "rate_limit_error"),
        success("ok"),
    ]).await;
    let started = Instant::now();
    let (result, events) = run(&fake, FAST, 5000, None).await;
    assert_eq!(result.unwrap().text, "ok");
    assert_eq!(attempts(&fake), 2);
    assert_eq!(waits(&events), vec![(1, 3, Duration::from_secs(1))]);
    assert!(started.elapsed() >= Duration::from_secs(1));
}

#[tokio::test]
async fn server_errors_back_off_exponentially_until_success() {
    let fake = serve_sequence(vec![
        api_error("500 Internal Server Error", "", "api_error"),
        http("502 Bad Gateway", "", "<html>bad gateway</html>"),
        api_error("503 Service Unavailable", "", "api_error"),
        success("ok"),
    ]).await;
    let (result, events) = run(&fake, FAST, 2000, None).await;
    assert_eq!(result.unwrap().text, "ok");
    assert_eq!(attempts(&fake), 4);
    let w = waits(&events);
    assert_eq!(w.iter().map(|w| w.0).collect::<Vec<_>>(), [1, 2, 3]);
    for (wait, full) in w.iter().zip([20, 40, 80]) {
        assert_backoff(wait.2, full);
    }
}

#[tokio::test]
async fn every_listed_transient_status_is_retried() {
    for status in [408, 409, 429, 500, 502, 503, 504, 529] {
        let fake = serve_sequence(vec![api_error(&format!("{status} X"), "", "api_error"), success("ok")]).await;
        let (result, _) = run(&fake, FAST, 2000, None).await;
        assert_eq!(result.map(|r| r.text), Ok("ok".to_string()), "{status}");
        assert_eq!(attempts(&fake), 2, "{status}");
    }
}

#[tokio::test]
async fn gives_up_after_three_retries() {
    let fake = serve(overloaded()).await;
    let (result, events) = run(&fake, FAST, 2000, None).await;
    assert!(matches!(result, Err(ClaudeStreamError::Http { status: 529, .. })), "{result:?}");
    assert_eq!(attempts(&fake), 4);
    assert_eq!(waits(&events).len(), 3);
}

#[tokio::test]
async fn non_retryable_statuses_fail_immediately() {
    for status in [400, 401, 403, 404, 413] {
        let fake = serve(api_error(&format!("{status} X"), "", "invalid_request_error")).await;
        let (result, events) = run(&fake, FAST, 2000, None).await;
        assert!(matches!(result, Err(ClaudeStreamError::Http { status: s, .. }) if s == status), "{status}: {result:?}");
        assert_eq!(attempts(&fake), 1, "{status}");
        assert!(events.is_empty(), "{status}");
    }
}

const SPEND_LIMIT: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Monthly spend limit reached","details":{"error_code":"enforced_spend_limit_reached"}}}"#;

#[tokio::test]
async fn spend_limit_429_fails_immediately_with_its_message() {
    let fake = serve(http("429 Too Many Requests", "", SPEND_LIMIT)).await;
    let (result, events) = run(&fake, FAST, 2000, None).await;
    assert!(result.unwrap_err().to_string().contains("Monthly spend limit reached"));
    assert_eq!(attempts(&fake), 1);
    assert!(events.is_empty());
}

#[tokio::test]
async fn spend_limit_code_with_retry_after_is_still_retried() {
    let fake = serve_sequence(vec![
        http("429 Too Many Requests", "retry-after: 0\r\n", SPEND_LIMIT),
        success("ok"),
    ]).await;
    let (result, _) = run(&fake, FAST, 2000, None).await;
    assert_eq!(result.unwrap().text, "ok");
    assert_eq!(attempts(&fake), 2);
}

#[tokio::test]
async fn idle_stall_is_retried() {
    let fake = serve_sequence(vec![
        vec![ok_head(), start(), text_delta("stalled "), Step::Sleep(5000)],
        success("fresh"),
    ]).await;
    let (result, events) = run(&fake, FAST, 150, None).await;
    // The retry restarts the whole request: nothing from the stalled one survives.
    assert_eq!(result.unwrap().text, "fresh");
    assert_eq!(attempts(&fake), 2);
    assert_eq!(waits(&events).len(), 1);
}

fn mid_stream_error(error_type: &str) -> Vec<Step> {
    vec![ok_head(), start(), text_delta("partial "),
        sse("error", serde_json::json!({"type":"error","error":{"type":error_type,"message":"boom"}}))]
}

#[tokio::test]
async fn transient_mid_stream_errors_restart_the_whole_request() {
    for kind in ["overloaded_error", "api_error", "rate_limit_error", "timeout_error"] {
        let fake = serve_sequence(vec![mid_stream_error(kind), success("whole")]).await;
        let (result, _) = run(&fake, FAST, 2000, None).await;
        assert_eq!(result.map(|r| r.text), Ok("whole".to_string()), "{kind}");
        assert_eq!(attempts(&fake), 2, "{kind}");
    }
}

#[tokio::test]
async fn non_transient_mid_stream_error_is_not_retried() {
    let fake = serve(mid_stream_error("invalid_request_error")).await;
    let (result, _) = run(&fake, FAST, 2000, None).await;
    assert!(matches!(result, Err(ClaudeStreamError::Stream { .. })), "{result:?}");
    assert_eq!(attempts(&fake), 1);
}

#[tokio::test]
async fn exhausted_mid_stream_errors_fail_without_returning_the_partial() {
    let fake = serve(mid_stream_error("overloaded_error")).await;
    let (result, _) = run(&fake, FAST, 2000, None).await;
    assert!(matches!(result, Err(ClaudeStreamError::Stream { .. })), "{result:?}");
    assert_eq!(attempts(&fake), 4);
}

#[tokio::test]
async fn dropped_connections_are_retried() {
    // Closed before any response, then closed mid-stream before a stop reason.
    let fake = serve_sequence(vec![vec![], vec![ok_head(), start(), text_delta("cut")], success("ok")]).await;
    let (result, _) = run(&fake, FAST, 2000, None).await;
    assert_eq!(result.unwrap().text, "ok");
    assert_eq!(attempts(&fake), 3);
}

#[tokio::test]
async fn cancel_during_backoff_stops_at_once() {
    let fake = serve(api_error("429 Too Many Requests", "retry-after: 30\r\n", "rate_limit_error")).await;
    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        canceller.cancel();
    });
    let started = Instant::now();
    let (result, events) = run(&fake, FAST, 5000, Some(&token)).await;
    assert_eq!(result, Err(ClaudeStreamError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(attempts(&fake), 1);
    assert_eq!(waits(&events), vec![(1, 3, Duration::from_secs(30))]);
}

#[tokio::test]
async fn stalled_error_body_does_not_hang_the_retry_loop() {
    let stalled = vec![Step::Write("HTTP/1.1 529 Overloaded\r\ncontent-length: 100\r\n\r\n".into()), Step::Sleep(10_000)];
    let fake = serve_sequence(vec![stalled, success("ok")]).await;
    let started = Instant::now();
    let (result, _) = run(&fake, FAST, 150, None).await;
    assert_eq!(result.unwrap().text, "ok");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn waits_are_capped_even_when_retry_after_asks_for_more() {
    let policy = RetryPolicy::default();
    assert_eq!((policy.max_retries, policy.max_delay), (3, Duration::from_secs(60)));
    assert_eq!(policy.delay(1, Some(Duration::from_secs(600)), 1.0), Duration::from_secs(60));
    assert_eq!(policy.delay(30, None, 1.0), Duration::from_secs(60));
    // Jitter spans half to the whole exponential step.
    assert_eq!(policy.delay(3, None, 0.0), policy.base_delay * 2);
    assert_eq!(policy.delay(3, None, 1.0), policy.base_delay * 4);
}

#[tokio::test]
async fn summary_entry_point_retries_and_reports_progress() {
    let fake = serve_sequence(vec![overloaded(), success("# Notes")]).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let out = observe_retries(
        Arc::new(move |e: RetryEvent| sink.lock().unwrap().push(e.progress())),
        crate::summary::llm_client::generate_claude(
            shared_http_client(), &fake.url, "claude-sonnet-4-5", "sk-test", "sys", "transcript", None, None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(out, "# Notes");
    assert_eq!(attempts(&fake), 2);
    let events = events.lock().unwrap();
    assert_eq!(events[0].0, "retrying");
    assert!(events[0].1.contains("retrying (1 of 3)"), "{}", events[0].1);
    assert_eq!(events[1].0, "generating");
}
