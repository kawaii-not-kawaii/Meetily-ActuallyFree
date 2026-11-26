# Claude model limits and summary length (spec #6, ticket #12)

## Owners

- `frontend/src-tauri/src/anthropic/anthropic.rs`: Models API discovery, caches,
  and the Tauri commands `get_anthropic_models` and `get_claude_model_limit`.
- `frontend/src-tauri/src/summary/claude_limits.rs`: pure rules (live vs.
  catalog, save-time check, use-time clamp) and the bundled catalog.
- `frontend/src/lib/claude-output-limits.json`: catalog data. Only Rust matches
  model IDs against it; TypeScript reads the fallback model list from it.
- `frontend/src/components/ClaudeSummaryLengthField.tsx`: the settings field.

## Discovery

The list request pages through `GET /v1/models` (`limit=1000`, then `after_id`
while `has_more`). A failed page or a cursor that does not advance fails the
whole list, which then falls back to the bundled suggestions. Each entry keeps
`max_tokens`, `max_input_tokens` and the raw `capabilities` tree.

Limits are cached per model for 5 minutes, separately from the list cache. A
successful list fills that cache; a miss does `GET /v1/models/{id}` (5 s
timeout). Failures are not cached.

## Resolution

1. Live `max_tokens`, when present and non-zero.
2. Catalog (longest prefix), when there is no key, the call fails, the model is
   not listed, or the response has no limit.
3. Otherwise 8192, with `source: "unknown"`; the settings field says so.

`max_input_tokens` comes only from live data (for the context guard, ticket #17).

## Summary length

- No cap: request the model maximum. There is no application clamp.
- `api_save_model_config` rejects a Claude cap outside 1..limit, using the key
  being saved or the stored one. Without a cap it makes no request. Other
  providers only require a positive value.
- `summary/service.rs` clamps a stored cap to the current model's limit and
  passes the result to every Claude pass. That effective value is part of the
  summary cache key, so a summary cached under the old 8192 default is
  regenerated once when the model maximum differs.
- `generate_claude` sends the value it is given. Only a caller that passes none
  gets the catalog maximum. The live assistant passes its own small limit.

## Tests and limits

`cargo test -p meetily --lib claude` covers resolution, pagination, caching,
fallback and the save/use cap rules against a local fake Models API.
`tests/lib/claude-output-tokens.test.ts` and
`tests/components/claude-summary-length.test.tsx` cover the UI. Not verified
against the real API (ticket #10): live response fields, pagination with a real
account, and whether input plus `max_tokens` is checked against the window.
While the Models API is unreachable, a cap above the catalog value (or above
8192 for an uncatalogued model) is rejected on save and clamped at use.
