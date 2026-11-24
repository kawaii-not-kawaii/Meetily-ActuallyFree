# Claude summary streaming (spec #6, ticket #7)

Owner: `frontend/src-tauri/src/summary/claude_stream/` (client + fake-server
tests), called from `summary/llm_client.rs::generate_claude`.

## Data flow

`generate_summary` (Claude provider) → `generate_claude` → `stream_message`:
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

## Stop reasons

`end_turn`/`stop_sequence` succeed; `max_tokens`, `refusal` (with category),
`pause_turn`, `tool_use`, unknown or missing fail with the reason named.
Retry-at-higher-limit, partial save, transient-error retry and usage display
are later tickets of the spec.

## Tests

`cargo test -p meetily --lib summary::claude_stream` runs against a local
fake SSE server (no API key): streaming, UTF-8 split across chunks, idle stall,
ping resets idle, cancel, error event, HTTP error, stop-reason policy, shared
client. Not verified against the real API (spec ticket #10).
