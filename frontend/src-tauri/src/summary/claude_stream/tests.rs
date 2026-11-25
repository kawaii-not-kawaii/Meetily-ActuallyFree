//! Fake-SSE-server tests: no network, no API key.
use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub(super) enum Step {
    Write(String),
    Sleep(u64),
}

pub(super) struct Fake {
    pub(super) url: String,
    pub(super) requests: Arc<Mutex<Vec<String>>>,
}

pub(super) fn sse(event: &str, data: Value) -> Step {
    Step::Write(format!("event: {event}\ndata: {data}\n\n"))
}

pub(super) fn ok_head() -> Step {
    Step::Write("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n".into())
}

pub(super) fn text_delta(t: &str) -> Step {
    sse("content_block_delta", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":t}}))
}

pub(super) fn finish(reason: &str) -> Vec<Step> {
    vec![
        sse("message_delta", serde_json::json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":12}})),
        sse("message_stop", serde_json::json!({"type":"message_stop"})),
    ]
}

pub(super) fn start() -> Step {
    sse("message_start", serde_json::json!({"type":"message_start","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":40,"cache_creation_input_tokens":5,"output_tokens":1}}}))
}

/// Serves one scripted response per connection (script is replayed for every
/// connection; the socket closes after the last step unless the last step is a
/// long sleep).
pub(super) async fn serve(script: Vec<Step>) -> Fake {
    serve_sequence(vec![script]).await
}

/// Like `serve`, but the nth connection gets the nth script; the last script
/// repeats for any further connection.
pub(super) async fn serve_sequence(scripts: Vec<Vec<Step>>) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let reqs = requests.clone();
    let scripts: Vec<Arc<Vec<Step>>> = scripts.into_iter().map(Arc::new).collect();
    tokio::spawn(async move {
        for n in 0.. {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let script = scripts[usize::min(n, scripts.len() - 1)].clone();
            let reqs = reqs.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 { return; }
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h].to_lowercase().split("content-length:").nth(1)
                            .and_then(|r| r.lines().next()).and_then(|l| l.trim().parse::<usize>().ok()).unwrap_or(0);
                        if buf.len() >= h + 4 + len {
                            reqs.lock().unwrap().push(text);
                            break;
                        }
                    }
                }
                for step in script.iter() {
                    match step {
                        Step::Write(s) => { if sock.write_all(s.as_bytes()).await.is_err() { return; } let _ = sock.flush().await; }
                        Step::Sleep(ms) => tokio::time::sleep(Duration::from_millis(*ms)).await,
                    }
                }
            });
        }
    });
    Fake { url, requests }
}

async fn run(fake: &Fake, idle_ms: u64, cancel: Option<&CancellationToken>) -> Result<ClaudeStreamResult, ClaudeStreamError> {
    stream_message(
        &Client::new(), &fake.url, HeaderMap::new(),
        serde_json::json!({"model":"claude-x","max_tokens":10,"messages":[]}),
        Duration::from_millis(idle_ms), cancel,
    ).await
}

#[tokio::test]
async fn streams_text_stop_reason_and_usage() {
    let mut s = vec![ok_head(), start(), sse("ping", serde_json::json!({"type":"ping"})), text_delta("Hel"), text_delta("lo ✓")];
    s.extend(finish("end_turn"));
    let fake = serve(s).await;
    let r = run(&fake, 2000, None).await.unwrap();
    assert_eq!(r.text, "Hello ✓");
    assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(r.usage, ClaudeUsage { input_tokens: 100, cache_read_input_tokens: 40, cache_creation_input_tokens: 5, output_tokens: 12 });
    assert_eq!(complete_text(r).unwrap(), "Hello ✓");
    let req = fake.requests.lock().unwrap()[0].clone();
    assert!(req.contains("\"stream\":true"), "request must be streamed: {req}");
}

#[tokio::test]
async fn multibyte_split_across_chunks_is_preserved() {
    let line = format!("event: content_block_delta\ndata: {}\n\n",
        serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"é日"}}));
    let bytes = line.into_bytes();
    let cut = bytes.iter().position(|b| *b == 0xC3).unwrap() + 1; // mid-"é"
    // Raw socket writes can't split a String, so test the parser directly.
    let mut p = SseParser::default();
    assert!(p.push(&bytes[..cut]).is_empty());
    let ev = p.push(&bytes[cut..]);
    assert_eq!(ev.len(), 1);
    let mut r = ClaudeStreamResult::default();
    apply_event(&mut r, &ev[0]).unwrap();
    assert_eq!(r.text, "é日");
}

#[tokio::test]
async fn idle_stall_times_out_naming_the_period() {
    let fake = serve(vec![ok_head(), start(), text_delta("a"), Step::Sleep(5000)]).await;
    let err = run(&fake, 150, None).await.unwrap_err();
    assert_eq!(err, ClaudeStreamError::IdleTimeout(Duration::from_millis(150)));
    assert!(err.to_string().contains("idle timeout"));
    assert!(!err.to_string().contains("60 seconds"));
}

#[tokio::test]
async fn any_bytes_including_ping_reset_the_idle_clock() {
    let ping = || sse("ping", serde_json::json!({"type":"ping"}));
    let mut s = vec![ok_head(), start()];
    for _ in 0..5 { s.push(Step::Sleep(100)); s.push(ping()); }
    s.push(text_delta("done"));
    s.extend(finish("end_turn"));
    let fake = serve(s).await;
    // Total ~500 ms, idle limit 300 ms: only passes if pings reset the timer.
    assert_eq!(run(&fake, 300, None).await.unwrap().text, "done");
}

#[tokio::test]
async fn cancel_during_stream_returns_promptly() {
    let fake = serve(vec![ok_head(), start(), text_delta("a"), Step::Sleep(10_000)]).await;
    let token = CancellationToken::new();
    let t2 = token.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(100)).await; t2.cancel(); });
    let started = std::time::Instant::now();
    let err = run(&fake, 60_000, Some(&token)).await.unwrap_err();
    assert_eq!(err, ClaudeStreamError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn mid_stream_error_event_is_typed() {
    let fake = serve(vec![ok_head(), start(), text_delta("partial"),
        sse("error", serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}))]).await;
    match run(&fake, 2000, None).await.unwrap_err() {
        ClaudeStreamError::Stream { error_type, message } => {
            assert_eq!(error_type, "overloaded_error");
            assert_eq!(message, "Overloaded");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn http_error_surfaces_body() {
    let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#;
    let fake = serve(vec![Step::Write(format!("HTTP/1.1 400 Bad Request\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()))]).await;
    match run(&fake, 2000, None).await.unwrap_err() {
        ClaudeStreamError::Http { status, body, .. } => { assert_eq!(status, 400); assert!(body.contains("bad")); }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn stop_reasons_policy() {
    for ok in ["end_turn", "stop_sequence"] {
        let mut s = vec![ok_head(), start(), text_delta("summary")];
        s.extend(finish(ok));
        let fake = serve(s).await;
        assert_eq!(complete_text(run(&fake, 2000, None).await.unwrap()).unwrap(), "summary");
    }
    for bad in ["max_tokens", "refusal", "pause_turn", "tool_use", "weird"] {
        let mut s = vec![ok_head(), start(), text_delta("partial")];
        s.extend(finish(bad));
        let fake = serve(s).await;
        let err = complete_text(run(&fake, 2000, None).await.unwrap()).unwrap_err();
        assert!(err.contains(bad) || bad == "max_tokens" && err.contains("output limit"), "{bad}: {err}");
    }
    // Missing stop reason (stream ended early) never succeeds.
    let fake = serve(vec![ok_head(), start(), text_delta("partial")]).await;
    assert!(matches!(run(&fake, 2000, None).await, Err(ClaudeStreamError::Connect(_))));
}

#[test]
fn refusal_names_category() {
    let r = ClaudeStreamResult {
        text: "x".into(), stop_reason: Some("refusal".into()),
        stop_details: Some(serde_json::json!({"type":"refusal","category":"cyber","explanation":"nope"})),
        ..Default::default()
    };
    let e = complete_text(r).unwrap_err();
    assert!(e.contains("cyber") && e.contains("nope"), "{e}");
}

#[test]
fn shared_client_is_one_instance() {
    assert!(std::ptr::eq(shared_http_client(), shared_http_client()));
}

#[tokio::test]
async fn summary_entry_point_streams_end_to_end() {
    let mut s = vec![ok_head(), start(), text_delta("# Notes\n"), text_delta("All good")];
    s.extend(finish("end_turn"));
    let fake = serve(s).await;
    let out = crate::summary::llm_client::generate_claude(
        shared_http_client(), &fake.url, "claude-sonnet-4-5", "sk-test", "sys", "transcript", None, None,
    ).await.unwrap();
    assert_eq!(out, "# Notes\nAll good");
    let req = fake.requests.lock().unwrap()[0].to_lowercase();
    assert!(req.contains("x-api-key: sk-test") && req.contains("\"stream\":true") && req.contains("\"system\":\"sys\""));
}
