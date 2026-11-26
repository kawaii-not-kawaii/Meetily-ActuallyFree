//! Fake Models API tests: no network, no API key. Model IDs are unique per
//! test because the per-model cache is process-wide.
use super::*;
use serde_json::json;
use crate::summary::claude_limits::LimitSource;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Fake {
    base: String,
    /// Request targets (path and query) in arrival order.
    targets: Arc<Mutex<Vec<String>>>,
    heads: Arc<Mutex<Vec<String>>>,
}

/// Answers each GET by exact target; anything unrouted is a 404.
async fn serve(routes: Vec<(&'static str, u16, Value)>) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let targets = Arc::new(Mutex::new(Vec::new()));
    let heads = Arc::new(Mutex::new(Vec::new()));
    let (t, h) = (targets.clone(), heads.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut tmp).await.unwrap_or(0);
                if n == 0 { break; }
                buf.extend_from_slice(&tmp[..n]);
            }
            let head = String::from_utf8_lossy(&buf).to_string();
            let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
            let (status, body) = routes.iter().find(|r| r.0 == target)
                .map(|r| (r.1, r.2.to_string()))
                .unwrap_or((404, r#"{"type":"error","error":{"type":"not_found_error"}}"#.into()));
            t.lock().unwrap().push(target);
            h.lock().unwrap().push(head.to_lowercase());
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(response.as_bytes()).await;
        }
    });
    Fake { base, targets, heads }
}

fn model(id: &str, max_tokens: u32) -> Value {
    json!({"type":"model","id":id,"display_name":id,"max_tokens":max_tokens,"max_input_tokens":1_000_000,
        "capabilities":{"effort":{"supported":true,"low":{"supported":true}}}})
}

fn client() -> &'static reqwest::Client {
    crate::summary::claude_stream::shared_http_client()
}

#[tokio::test]
async fn model_list_follows_pagination_and_keeps_limits_and_capabilities() {
    let fake = serve(vec![
        ("/v1/models?limit=1000", 200, json!({"data":[model("claude-page-a", 128000), model("not-a-chat-model", 1)],"has_more":true,"last_id":"claude-page-a"})),
        ("/v1/models?limit=1000&after_id=claude-page-a", 200, json!({"data":[model("claude-page-b", 64000)],"has_more":false,"last_id":"claude-page-b"})),
    ]).await;
    let models = fetch_models(client(), &fake.base, "sk-test").await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["claude-page-a", "claude-page-b"]);
    assert_eq!((models[1].max_tokens, models[1].max_input_tokens), (Some(64000), Some(1_000_000)));
    assert_eq!(models[0].capabilities.as_ref().unwrap()["effort"]["low"]["supported"], true);
    let head = fake.heads.lock().unwrap()[0].clone();
    assert!(head.contains("x-api-key: sk-test") && head.contains("anthropic-version: 2023-06-01"), "{head}");

    // The list also filled the per-model cache: no retrieve request follows.
    let limit = model_limit(client(), &fake.base, Some("sk-test"), "claude-page-b").await;
    assert_eq!((limit.max_output_tokens, limit.source), (64000, LimitSource::Live));
    assert_eq!(fake.targets.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_failing_later_page_or_stuck_cursor_fails_the_list() {
    let broken = serve(vec![
        ("/v1/models?limit=1000", 200, json!({"data":[model("claude-broken-a", 1000)],"has_more":true,"last_id":"claude-broken-a"})),
    ]).await;
    assert!(fetch_models(client(), &broken.base, "k").await.is_err());
    let stuck = serve(vec![
        ("/v1/models?limit=1000", 200, json!({"data":[],"has_more":true,"last_id":"x"})),
        ("/v1/models?limit=1000&after_id=x", 200, json!({"data":[],"has_more":true,"last_id":"x"})),
    ]).await;
    assert!(fetch_models(client(), &stuck.base, "k").await.is_err());
    assert_eq!(stuck.targets.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn retrieve_supplies_the_live_limit_and_is_cached() {
    // The catalog says 128000 for this family; live data wins.
    let fake = serve(vec![("/v1/models/claude-sonnet-5-retrieve", 200, model("claude-sonnet-5-retrieve", 200_000))]).await;
    for _ in 0..2 {
        let limit = model_limit(client(), &fake.base, Some("k"), "claude-sonnet-5-retrieve").await;
        assert_eq!((limit.max_output_tokens, limit.max_input_tokens, limit.source), (200_000, Some(1_000_000), LimitSource::Live));
    }
    assert_eq!(fake.targets.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn catalog_covers_unlisted_models_failed_calls_and_missing_keys() {
    let unlisted = serve(vec![]).await;
    let limit = model_limit(client(), &unlisted.base, Some("k"), "claude-haiku-4-5-unlisted").await;
    assert_eq!((limit.max_output_tokens, limit.source), (64_000, LimitSource::Catalog));
    let limit = model_limit(client(), &unlisted.base, Some("k"), "claude-nowhere").await;
    assert_eq!((limit.max_output_tokens, limit.source), (8192, LimitSource::Unknown));

    let failing = serve(vec![("/v1/models/claude-opus-5-5-failing", 500, json!({}))]).await;
    let limit = model_limit(client(), &failing.base, Some("k"), "claude-opus-5-5-failing").await;
    assert_eq!((limit.max_output_tokens, limit.source), (128_000, LimitSource::Catalog));
    // Failures are not cached, so a recovered API is used on the next call.
    model_limit(client(), &failing.base, Some("k"), "claude-opus-5-5-failing").await;
    assert_eq!(failing.targets.lock().unwrap().len(), 2);

    // Port 1 is unreachable.
    let limit = model_limit(client(), "http://127.0.0.1:1", Some("k"), "claude-opus-5-5-offline").await;
    assert_eq!(limit.source, LimitSource::Catalog);

    let before = unlisted.targets.lock().unwrap().len();
    model_limit(client(), &unlisted.base, None, "claude-haiku-4-5-nokey").await;
    model_limit(client(), &unlisted.base, Some("  "), "claude-haiku-4-5-nokey").await;
    assert_eq!(unlisted.targets.lock().unwrap().len(), before);
}

#[tokio::test]
async fn model_id_cannot_change_the_request_path() {
    let fake = serve(vec![]).await;
    model_limit(client(), &fake.base, Some("k"), "claude-x/../../admin?a=1").await;
    let targets = fake.targets.lock().unwrap().clone();
    assert!(targets.iter().all(|t| t.starts_with("/v1/models/claude-x%2F") && !t.contains('?')), "{targets:?}");
}

#[tokio::test]
async fn cap_is_checked_against_the_live_limit_at_save_and_clamped_after_a_model_change() {
    let fake = serve(vec![
        ("/v1/models/claude-save-big", 200, model("claude-save-big", 128_000)),
        ("/v1/models/claude-save-small", 200, model("claude-save-small", 32_000)),
    ]).await;
    let base = fake.base.as_str();
    check_summary_cap(client(), base, Some("k"), "claude-save-big", Some(100_000)).await.unwrap();
    let error = check_summary_cap(client(), base, Some("k"), "claude-save-small", Some(100_000)).await.unwrap_err();
    assert!(error.contains("32000"), "{error}");
    assert_eq!(summary_max_tokens(client(), base, Some("k"), "claude-save-big", Some(100_000)).await, 100_000);
    assert_eq!(summary_max_tokens(client(), base, Some("k"), "claude-save-small", Some(100_000)).await, 32_000);
    assert_eq!(summary_max_tokens(client(), base, Some("k"), "claude-save-big", None).await, 128_000);
}

#[tokio::test]
async fn saving_without_a_cap_makes_no_models_request() {
    let fake = serve(vec![]).await;
    check_summary_cap(client(), &fake.base, Some("k"), "claude-nocap", None).await.unwrap();
    assert!(fake.targets.lock().unwrap().is_empty());
}
