use super::*;

fn assert_close(actual: f64, expected: f64) {
    let tolerance = expected.abs().max(1.0) * 1e-12;
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected:.12}, got {actual:.12}"
    );
}

fn assert_valid_rate(model: &str, name: &str, rate: f64) {
    assert!(rate.is_finite() && rate >= 0.0, "{model} {name}: {rate}");
}

fn assert_valid_optional_rate(model: &str, name: &str, rate: Option<f64>) {
    if let Some(rate) = rate {
        assert_valid_rate(model, name, rate);
    }
}

fn assert_claude_rates_equal(actual: ClaudePricing, expected: ClaudePricing, model: &str) {
    assert_close(actual.input_cost_per_token, expected.input_cost_per_token);
    assert_close(actual.output_cost_per_token, expected.output_cost_per_token);
    assert_close(
        actual.cache_creation_input_cost_per_token,
        expected.cache_creation_input_cost_per_token,
    );
    assert_close(
        actual.cache_read_input_cost_per_token,
        expected.cache_read_input_cost_per_token,
    );
    assert_eq!(
        actual.threshold_tokens, expected.threshold_tokens,
        "{model}"
    );
    assert_eq!(
        actual.input_cost_per_token_above_threshold, expected.input_cost_per_token_above_threshold,
        "{model}"
    );
    assert_eq!(
        actual.output_cost_per_token_above_threshold,
        expected.output_cost_per_token_above_threshold,
        "{model}"
    );
    assert_eq!(
        actual.cache_creation_input_cost_per_token_above_threshold,
        expected.cache_creation_input_cost_per_token_above_threshold,
        "{model}"
    );
    assert_eq!(
        actual.cache_read_input_cost_per_token_above_threshold,
        expected.cache_read_input_cost_per_token_above_threshold,
        "{model}"
    );
}

#[test]
fn every_codex_table_entry_has_valid_rates_and_resolves_to_a_price() {
    assert!(!CODEX_PRICING.is_empty());
    for (&model, pricing) in CODEX_PRICING.iter() {
        let normalized = CostUsagePricing::normalize_codex_model(model);
        assert!(
            CODEX_PRICING.contains_key(normalized.as_str()),
            "{model} normalizes to missing entry {normalized}"
        );
        for (name, rate) in [
            ("input", pricing.input_cost_per_token),
            ("output", pricing.output_cost_per_token),
            ("cache read", pricing.cache_read_input_cost_per_token),
        ] {
            assert_valid_rate(model, name, rate);
        }
        if let Some(long_context) = pricing.long_context {
            for (name, rate) in [
                ("long-context input", long_context.input_cost_per_token),
                ("long-context output", long_context.output_cost_per_token),
                (
                    "long-context cache read",
                    long_context.cache_read_input_cost_per_token,
                ),
            ] {
                assert_valid_rate(model, name, rate);
            }
        }
        // SBS-710 tracks the known gpt-5.1-codex-mini normalization bug.
        // Keep its behavior fix separate from this tests-only PR.
        assert!(
            CostUsagePricing::codex_cost_usd(model, 0, 0, 0).is_some(),
            "{model}"
        );
    }
}

#[test]
fn every_claude_table_entry_is_reachable_and_aliases_match() {
    let date = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
    assert!(!CLAUDE_PRICING.is_empty());
    for (&model, pricing) in CLAUDE_PRICING.iter() {
        for (name, rate) in [
            ("input", pricing.input_cost_per_token),
            ("output", pricing.output_cost_per_token),
            (
                "cache creation",
                pricing.cache_creation_input_cost_per_token,
            ),
            ("cache read", pricing.cache_read_input_cost_per_token),
        ] {
            assert_valid_rate(model, name, rate);
        }
        for (name, rate) in [
            (
                "above-threshold input",
                pricing.input_cost_per_token_above_threshold,
            ),
            (
                "above-threshold output",
                pricing.output_cost_per_token_above_threshold,
            ),
            (
                "above-threshold cache creation",
                pricing.cache_creation_input_cost_per_token_above_threshold,
            ),
            (
                "above-threshold cache read",
                pricing.cache_read_input_cost_per_token_above_threshold,
            ),
        ] {
            assert_valid_optional_rate(model, name, rate);
        }
        if let Some(threshold) = pricing.threshold_tokens {
            assert!(threshold >= 0, "{model} threshold: {threshold}");
        }
        let normalized = CostUsagePricing::normalize_claude_model(model);
        let resolved = CLAUDE_PRICING
            .get(normalized.as_str())
            .unwrap_or_else(|| panic!("{model} normalizes to missing entry {normalized}"));
        assert_claude_rates_equal(*resolved, *pricing, model);
        assert!(
            CostUsagePricing::claude_cost_usd_on_date(model, 0, 0, 0, 0, date).is_some(),
            "{model}"
        );
    }
}

#[test]
fn codex_prices_input_cache_and_output_as_separate_channels() {
    let pricing = CODEX_PRICING["gpt-5"];
    let input_only = CostUsagePricing::codex_cost_usd("gpt-5", 1_000, 0, 0).unwrap();
    let cache_only = CostUsagePricing::codex_cost_usd("gpt-5", 1_000, 1_000, 0).unwrap();
    let output_only = CostUsagePricing::codex_cost_usd("gpt-5", 0, 0, 1_000).unwrap();
    assert_close(input_only, 1_000.0 * pricing.input_cost_per_token);
    assert_close(
        cache_only,
        1_000.0 * pricing.cache_read_input_cost_per_token,
    );
    assert_close(output_only, 1_000.0 * pricing.output_cost_per_token);
}

#[test]
fn codex_cached_input_is_clamped_to_total_input() {
    let pricing = CODEX_PRICING["gpt-5"];
    let cost = CostUsagePricing::codex_cost_usd("gpt-5", 10, 100, 0).unwrap();
    assert_close(cost, 10.0 * pricing.cache_read_input_cost_per_token);
}

#[test]
fn claude_prices_all_four_token_channels_separately() {
    let pricing = CLAUDE_PRICING["claude-haiku-4-5"];
    let date = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
    let cost = CostUsagePricing::claude_cost_usd_on_date(
        "claude-haiku-4-5",
        1_000,
        2_000,
        3_000,
        4_000,
        date,
    )
    .unwrap();
    assert_close(
        cost,
        1_000.0 * pricing.input_cost_per_token
            + 2_000.0 * pricing.cache_read_input_cost_per_token
            + 3_000.0 * pricing.cache_creation_input_cost_per_token
            + 4_000.0 * pricing.output_cost_per_token,
    );
}

#[test]
fn claude_tier_boundary_prices_only_tokens_above_the_threshold_at_premium() {
    let pricing = CLAUDE_PRICING["claude-sonnet-4-5"];
    let threshold = pricing.threshold_tokens.unwrap();
    let date = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
    let at_threshold =
        CostUsagePricing::claude_cost_usd_on_date("claude-sonnet-4-5", threshold, 0, 0, 0, date)
            .unwrap();
    let one_over = CostUsagePricing::claude_cost_usd_on_date(
        "claude-sonnet-4-5",
        threshold + 1,
        0,
        0,
        0,
        date,
    )
    .unwrap();
    assert_close(
        at_threshold,
        threshold as f64 * pricing.input_cost_per_token,
    );
    assert_close(
        one_over - at_threshold,
        pricing.input_cost_per_token_above_threshold.unwrap(),
    );
}

#[test]
fn sonnet_5_uses_rates_effective_on_the_usage_date() {
    let before = NaiveDate::from_ymd_opt(2026, 8, 31).unwrap();
    let after = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
    assert_close(
        CostUsagePricing::claude_cost_usd_on_date("claude-sonnet-5", 1_000_000, 0, 0, 0, before)
            .unwrap(),
        2.0,
    );
    assert_close(
        CostUsagePricing::claude_cost_usd_on_date("claude-sonnet-5", 1_000_000, 0, 0, 0, after)
            .unwrap(),
        3.0,
    );
}

#[test]
fn zero_negative_and_unknown_usage_have_explicit_results() {
    let date = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
    assert_eq!(
        CostUsagePricing::codex_cost_usd("gpt-5", 0, 0, 0),
        Some(0.0)
    );
    assert_eq!(
        CostUsagePricing::claude_cost_usd_on_date("claude-haiku-4-5", -1, -2, -3, -4, date,),
        Some(0.0)
    );
    assert_eq!(
        CostUsagePricing::codex_cost_usd("definitely-not-a-model", 1, 1, 1),
        None
    );
    assert_eq!(
        CostUsagePricing::claude_cost_usd_on_date("definitely-not-a-model", 1, 1, 1, 1, date,),
        None
    );
}

#[test]
fn test_normalize_codex_model() {
    assert_eq!(CostUsagePricing::normalize_codex_model("gpt-5"), "gpt-5");
    assert_eq!(
        CostUsagePricing::normalize_codex_model("openai/gpt-5"),
        "gpt-5"
    );
    assert_eq!(
        CostUsagePricing::normalize_codex_model("gpt-5-codex"),
        "gpt-5"
    );
}

#[test]
fn test_normalize_claude_model() {
    assert_eq!(
        CostUsagePricing::normalize_claude_model("claude-sonnet-4-5"),
        "claude-sonnet-4-5"
    );
    assert_eq!(
        CostUsagePricing::normalize_claude_model("anthropic.claude-sonnet-4-5"),
        "claude-sonnet-4-5"
    );
}

#[test]
fn test_codex_cost() {
    let cost = CostUsagePricing::codex_cost_usd("gpt-5", 1000, 0, 500).unwrap();
    assert!((cost - 0.00625).abs() < 1e-10);
}

#[test]
fn test_claude_cost() {
    assert!(
        CostUsagePricing::claude_cost_usd("claude-haiku-4-5-20251001", 1000, 0, 0, 500).is_some()
    );
}

#[test]
fn test_opus_4_8_cost() {
    let cost = CostUsagePricing::claude_cost_usd("claude-opus-4-8", 1_000, 0, 0, 500).unwrap();
    assert!((cost - 0.0175).abs() < 1e-10);
}

#[test]
fn test_fable_5_cost() {
    let cost = CostUsagePricing::claude_cost_usd("claude-fable-5", 1_000, 0, 0, 500).unwrap();
    assert!((cost - 0.035).abs() < 1e-10);
}

#[test]
fn test_claude_input_cost_per_token() {
    assert_eq!(
        CostUsagePricing::claude_input_cost_per_token("claude-opus-4-8"),
        Some(5e-6)
    );
    assert_eq!(
        CostUsagePricing::claude_input_cost_per_token("claude-fable-5"),
        Some(1e-5)
    );
    assert_eq!(
        CostUsagePricing::claude_input_cost_per_token("totally-unknown-model"),
        None
    );
}

#[test]
fn test_format_model_name() {
    assert_eq!(
        CostUsagePricing::format_model_name("claude-3.5-sonnet"),
        "Sonnet 3.5"
    );
    assert_eq!(
        CostUsagePricing::format_model_name("claude-opus-4"),
        "Opus 4"
    );
    assert_eq!(CostUsagePricing::format_model_name("gpt-5"), "GPT-5");
}

#[test]
fn test_gpt54_mini_cost() {
    let cost = CostUsagePricing::codex_cost_usd("gpt-5.4-mini", 1000, 0, 500).unwrap();
    assert!((cost - 0.003).abs() < 1e-10);
}

#[test]
fn test_gpt54_nano_cost() {
    let cost = CostUsagePricing::codex_cost_usd("gpt-5.4-nano", 1000, 0, 500).unwrap();
    assert!((cost - 0.000825).abs() < 1e-10);
}

#[test]
fn test_normalize_gpt54_codex() {
    assert_eq!(
        CostUsagePricing::normalize_codex_model("gpt-5.4-mini-codex"),
        "gpt-5.4-mini"
    );
}

#[test]
fn test_gpt55_pricing() {
    assert_eq!(
        CostUsagePricing::normalize_codex_model("openai/gpt-5.5-2026-04-23"),
        "gpt-5.5"
    );
    assert_eq!(
        CostUsagePricing::normalize_codex_model("gpt-5.5-pro-2026-04-23"),
        "gpt-5.5-pro"
    );
    let cost = CostUsagePricing::codex_cost_usd("gpt-5.5", 1000, 500, 500).unwrap();
    assert!((cost - 0.01775).abs() < 1e-10);
}

#[test]
fn test_format_gpt54_mini() {
    assert_eq!(
        CostUsagePricing::format_model_name("gpt-5.4-mini"),
        "GPT-5.4 Mini"
    );
}

#[test]
fn test_opus_4_7_cost() {
    assert!(CostUsagePricing::claude_cost_usd("claude-opus-4-7", 1000, 0, 0, 500).is_some());
}

#[test]
fn test_sonnet_4_6_cost() {
    assert!(CostUsagePricing::claude_cost_usd("claude-sonnet-4-6", 1000, 0, 0, 500).is_some());
}

#[test]
fn test_gpt5_pro_cost() {
    let cost = CostUsagePricing::codex_cost_usd("gpt-5-pro", 1000, 0, 500).unwrap();
    assert!((cost - 0.075).abs() < 1e-10);
}

#[test]
fn test_gpt56_standard_pricing() {
    for (model, expected) in [
        ("gpt-5.6-sol", 0.0332),
        ("gpt-5.6-terra", 0.0166),
        ("gpt-5.6-luna", 0.00664),
    ] {
        let cost = CostUsagePricing::codex_cost_usd(model, 1_000, 400, 1_000);
        assert!((cost.unwrap() - expected).abs() < 1e-10, "{model}");
    }
}

#[test]
fn test_gpt56_long_context_pricing() {
    for (model, expected) in [
        ("gpt-5.6-sol", 45.272001),
        ("gpt-5.6-terra", 22.6360005),
        ("gpt-5.6-luna", 9.0544002),
    ] {
        let cost = CostUsagePricing::codex_cost_usd(model, 272_001, 272_001, 1_000_000);
        assert!((cost.unwrap() - expected).abs() < 1e-10, "{model}");
    }
}

#[test]
fn test_gpt56_context_threshold_is_exclusive() {
    for (model, expected) in [
        ("gpt-5.6-sol", 0.136),
        ("gpt-5.6-terra", 0.068),
        ("gpt-5.6-luna", 0.0272),
    ] {
        let cost = CostUsagePricing::codex_cost_usd(model, 272_000, 272_000, 0);
        assert!((cost.unwrap() - expected).abs() < 1e-10, "{model}");
    }
}

#[test]
fn test_normalize_gpt56_aliases() {
    for model in [
        "gpt-5.6",
        "openai/gpt-5.6",
        "gpt-5.6-codex",
        "gpt-5.6-2099-01-01",
        "openai/gpt-5.6-codex-2099-01-01",
    ] {
        assert_eq!(
            CostUsagePricing::normalize_codex_model(model),
            "gpt-5.6-sol",
            "{model}"
        );
    }
}

#[test]
fn test_codex_display_label() {
    assert_eq!(
        CostUsagePricing::codex_display_label("gpt-5.3-codex-spark"),
        Some("Research Preview")
    );
    assert_eq!(CostUsagePricing::codex_display_label("gpt-5.4"), None);
}
