# Claude summary streaming and retries (spec #6, tickets #7 and #11)

Owner: `frontend/src-tauri/src/summary/claude_stream/` (client + fake-server
tests), called from `summary/llm_client.rs::generate_claude`.

## Data flow

`generate_summary` (Claude provider) → `generate_claude` →
`stream_message_with_retry` (each attempt is one `stream_message` request):
POST `/v1/messages` with `stream: true` on the process-wide
`shared_http_client()` (also used by `summary/service.rs` and the live
assistant). SSE events fold into a `ClaudeStreamResult`: text from
`text_delta`, `stop_reason`/`stop_details` and cumulative usage from
`message_delta` (input/cache counts from `message_start`). A mid-stream `error`
event becomes `ClaudeStreamError::Stream`. `claude-cli` is untouched.

## Timeouts and cancel

No total timeout. Connect timeout 15 s (client builder); idle timeout 90 s
(`CLAUDE_IDLE_TIMEOUT`) resets on any received bytes, so `ping` events keep a
silent-reasoning stream alive. The user's `CancellationToken` drops the
response, closing the connection. Error text names the real idle period.
Non-Claude providers keep their per-request 300 s timeout (message corrected).

## Retries (`claude_stream/retry.rs`)

A retry restarts the whole streamed request; text from a failed attempt is never
returned or saved. Up to 3 retries (4 attempts).

- Retried: HTTP 408, 409, 429, 500, 502, 503, 504, 529; connection failures,
  including a stream that ends with no stop reason; idle timeout; mid-stream
  `error` events of type `overloaded_error`, `api_error`, `rate_limit_error` or
  `timeout_error`.
- Not retried: every other status (400, 401, 403, 404, 413, ...); a 429 with no
  `retry-after` whose `error.details.error_code` is
  `enforced_spend_limit_reached`; other stream error types; malformed events;
  cancel; and a completed response with any stop reason.
- Wait: 2 s, 4 s, 8 s, each jittered down to as little as half, raised to
  `retry-after` (seconds) when that is longer, and never over 60 s. A longer
  `retry-after` is cut to 60 s. Cancel ends a pending wait at once.
- Progress: `service.rs` wraps generation in `observe_retries`, which emits
  `summary-progress` with stage `retrying` ("retrying (n of 3)"), then
  `generating` once a retried request is accepted. `useSummaryGeneration` shows
  the message in the generating panel. The observer is a Tokio task-local, so
  work moved to `tokio::spawn` would retry without reporting. The 5 s status poll
  does not carry the notice, so a view opened mid-wait shows it only from the
  next retry event.
- The live assistant shares `generate_claude`, so it retries too, silently and
  without a cancel token.
- Logs: each response's status and `request-id` header; each retry's failure
  category and wait. These lines carry no bodies, prompts, transcripts or keys.

## Stop reasons

`end_turn`/`stop_sequence` succeed; `max_tokens`, `refusal` (with category),
`pause_turn`, `tool_use`, unknown or missing fail with the reason named.
Retry-at-higher-limit, partial save and usage display are later tickets of the
spec.

## Tests

`cargo test -p meetily --lib summary::claude_stream` runs against a local
fake SSE server (no API key): streaming, UTF-8 split across chunks, idle stall,
ping resets idle, cancel, error event, HTTP error, stop-reason policy, shared
client, and in `retry/tests.rs` the retryable and non-retryable cases, attempt
counts, waits, cancel during backoff and progress events. The UI notice is in
`tests/hooks/summary-recovery.test.tsx`. Not verified against the real API (spec ticket #10).
