use super::*;

fn live(max_tokens: Option<u32>, max_input_tokens: Option<u32>) -> Option<LiveLimits> {
    Some(LiveLimits { max_tokens, max_input_tokens })
}

#[test]
fn live_data_wins_over_the_catalog() {
    // The catalog says 64000 for Haiku 4.5; a different live value must win.
    let limit = resolve_claude_limit("claude-haiku-4-5", live(Some(70_000), Some(200_000)));
    assert_eq!(
        limit,
        ClaudeLimit { max_output_tokens: 70_000, max_input_tokens: Some(200_000), source: LimitSource::Live }
    );
    // A model newer than the app works at its full limit.
    assert_eq!(resolve_claude_limit("claude-next-9", live(Some(256_000), None)).max_output_tokens, 256_000);
}

#[test]
fn catalog_is_used_when_the_api_gives_no_usable_data() {
    for data in [None, live(None, None), live(Some(0), None)] {
        let limit = resolve_claude_limit("claude-haiku-4-5-20251001", data);
        assert_eq!((limit.max_output_tokens, limit.source), (64_000, LimitSource::Catalog));
    }
}

#[test]
fn model_unknown_to_both_keeps_8192_and_is_flagged() {
    let limit = resolve_claude_limit("claude-next-9", None);
    assert_eq!((limit.max_output_tokens, limit.source), (8192, LimitSource::Unknown));
}

#[test]
fn catalog_resolves_every_model_to_its_limit_by_longest_prefix() {
    let cases: &[(&str, u32)] = &[
        ("claude-3-opus-20240229", 4096), ("claude-3-5-sonnet-latest", 8192), ("claude-3-7-sonnet-latest", 64000),
        ("claude-sonnet-4-20250514", 64000), ("claude-sonnet-4-5-20250929", 64000), ("claude-sonnet-4-6", 128000),
        ("claude-sonnet-5", 128000), ("claude-sonnet-5-5", 128000),
        ("claude-haiku-4-5-20251001", 64000), ("claude-haiku-5", 128000), ("claude-haiku-5-5", 128000),
        ("claude-opus-4-1-20250805", 32000), ("claude-opus-4-20250514", 32000), ("claude-opus-4-5-20251101", 64000),
        ("claude-opus-4-6", 128000), ("claude-opus-4-7", 128000), ("claude-opus-4-8", 128000),
        ("claude-opus-5", 128000), ("claude-opus-5-5", 128000),
        ("claude-fable-5-2", 128000), ("claude-mythos-5-2", 128000),
    ];
    for (model, expected) in cases {
        let limit = resolve_claude_limit(model, None);
        assert_eq!((limit.max_output_tokens, limit.source), (*expected, LimitSource::Catalog), "{model}");
    }
    // A prefix only matches on a `-` boundary.
    assert_eq!(resolve_claude_limit("claude-sonnet-4-50-unknown", None).max_output_tokens, 64000);
    assert_eq!(resolve_claude_limit("claude-opus-50", None).source, LimitSource::Unknown);
}

#[test]
fn default_is_the_model_maximum_and_128k_is_not_clamped() {
    let limit = resolve_claude_limit("claude-sonnet-5-5", None);
    assert_eq!(effective_max_tokens(&limit, None), 128_000);
    assert_eq!(effective_max_tokens(&limit, Some(100_000)), 100_000);
    assert!(validate_cap("claude-sonnet-5-5", &limit, Some(128_000)).is_ok());
}

#[test]
fn cap_is_rejected_at_save_and_clamped_at_use() {
    let small = resolve_claude_limit("claude-haiku-4-5", None);
    assert!(validate_cap("claude-haiku-4-5", &small, None).is_ok());
    assert!(validate_cap("claude-haiku-4-5", &small, Some(64_000)).is_ok());
    for value in [0, -1, 64_001, i64::MAX] {
        let error = validate_cap("claude-haiku-4-5", &small, Some(value)).unwrap_err();
        assert!(error.contains("64000") && error.contains("claude-haiku-4-5"), "{error}");
    }
    // A cap saved for a 128K model survives a switch to a 64K model.
    assert_eq!(effective_max_tokens(&small, Some(128_000)), 64_000);
    assert_eq!(effective_max_tokens(&small, Some(i64::MAX)), 64_000);
    // Save rejects non-positive values; if one is stored anyway it means "unset".
    assert_eq!(effective_max_tokens(&small, Some(0)), 64_000);
}

#[test]
fn unknown_limit_error_says_so() {
    let limit = resolve_claude_limit("claude-next-9", None);
    let error = validate_cap("claude-next-9", &limit, Some(9000)).unwrap_err();
    assert!(error.contains("unknown"), "{error}");
}

#[test]
fn fallback_models_have_no_retired_entries() {
    let ids: Vec<&str> = claude_fallback_models().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5", "claude-haiku-4-5"]);
}
