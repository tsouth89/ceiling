//! Cost command implementation
//!
//! Scans local logs to calculate token costs for Codex, Claude, and Grok.

use clap::Args;

use super::usage::{OutputFormat, ProviderSelection};
use crate::core::ProviderId;
use crate::cost_scanner::{CostScanner, CostSummary};

/// Arguments for the cost command
#[derive(Args, Debug, Default)]
pub struct CostArgs {
    /// Provider to query (codex, claude, grok, cursor, gemini, copilot, all, both)
    #[arg(short, long)]
    pub provider: Option<String>,

    /// Output format: text or json
    #[arg(short, long, default_value = "text")]
    pub format: OutputFormat,

    /// Shorthand for --format json
    #[arg(long)]
    pub json: bool,

    /// Disable ANSI colors in text output
    #[arg(long = "no-color")]
    pub no_color: bool,

    /// Pretty-print JSON output
    #[arg(long)]
    pub pretty: bool,

    /// Number of days to scan (default: 30)
    #[arg(short, long, default_value = "30")]
    pub days: u32,

    /// Codex cost speed tier (ccusage parity): auto | standard | fast.
    ///
    /// `auto` (default) reads `service_tier` from `~/.codex/config.toml`.
    /// `priority` maps to fast (2× list rates), matching `ccusage codex --speed auto`.
    #[arg(long = "codex-speed", default_value = "auto")]
    pub codex_speed: String,
}

/// Run the cost command
pub async fn run(args: CostArgs) -> anyhow::Result<()> {
    let format = if args.json {
        OutputFormat::Json
    } else {
        args.format
    };

    let providers = ProviderSelection::from_arg(args.provider.as_deref())?;
    let settings = crate::settings::Settings::load();
    let provider_ids = providers.resolved_ids(&settings)?;
    let use_color = !args.no_color && is_terminal();
    let scanner = CostScanner::with_codex_speed(args.days, Some(args.codex_speed.as_str()));

    tracing::debug!(
        "Running cost command: providers={:?}, format={:?}, days={}, codex_speed={:?}",
        provider_ids,
        format,
        args.days,
        scanner.codex_speed()
    );

    let results = collect_results(&scanner, &provider_ids);

    match format {
        OutputFormat::Text => {
            print_text_output(&results, use_color, args.days);
        }
        OutputFormat::Json => {
            print_json_output(&results, args.pretty, args.days)?;
        }
    }

    Ok(())
}

const UNSUPPORTED_LOCAL_LOGS_HINT: &str = "(Only Codex, Claude, and Grok have local logs)";

/// Cost result for a provider
struct CostResult {
    provider: String,
    display_name: String,
    summary: CostSummary,
    supported: bool,
}

fn collect_results(scanner: &CostScanner, providers: &[ProviderId]) -> Vec<CostResult> {
    providers
        .iter()
        .copied()
        .map(|provider| match scanner.scan_provider(provider) {
            Some(summary) => CostResult {
                provider: provider.cli_name().to_string(),
                display_name: provider.display_name().to_string(),
                summary,
                supported: true,
            },
            None => CostResult {
                provider: provider.cli_name().to_string(),
                display_name: provider.display_name().to_string(),
                summary: CostSummary::default(),
                supported: false,
            },
        })
        .collect()
}

fn result_json(result: &CostResult, days: u32) -> serde_json::Value {
    if !result.supported {
        serde_json::json!({
            "provider": result.provider,
            "supported": false,
            "error": "Local cost scanning not available for this provider"
        })
    } else {
        serde_json::json!({
            "provider": result.provider,
            "supported": true,
            "days_scanned": days,
            "cost": {
                "total_usd": result.summary.total_cost_usd,
                "currency": "USD",
                "codex_speed": result.summary.codex_cost_speed,
                "codex_service_tier": result.summary.codex_service_tier
            },
            "tokens": {
                "input": result.summary.input_tokens,
                "output": result.summary.output_tokens,
                "cached": result.summary.cached_tokens,
                "cache_read": result.summary.cache_read_tokens,
                "cache_write": result.summary.cache_write_tokens
            },
            "sessions_count": result.summary.sessions_count,
            "by_model": result.summary.by_model,
            "by_effort": result.summary.by_effort,
            "by_effort_tokens": result.summary.by_effort_tokens.iter().map(|(bucket, counts)| {
                (bucket.clone(), serde_json::json!({
                    "input": counts.input_tokens,
                    "output": counts.output_tokens,
                    "cached": counts.cached_tokens,
                    "total": counts.total()
                }))
            }).collect::<serde_json::Map<_, _>>(),
            "period": {
                "start": result.summary.period_start.map(|d| d.to_string()),
                "end": result.summary.period_end.map(|d| d.to_string())
            }
        })
    }
}

/// Print text output
fn print_text_output(results: &[CostResult], use_color: bool, days: u32) {
    for (i, result) in results.iter().enumerate() {
        if use_color {
            println!(
                "\x1b[1m{} Cost (last {} days)\x1b[0m",
                result.display_name, days
            );
        } else {
            println!("{} Cost (last {} days)", result.display_name, days);
        }

        if !result.supported {
            println!("  Local cost scanning not available for this provider");
            println!("  {UNSUPPORTED_LOCAL_LOGS_HINT}");
        } else if result.summary.sessions_count == 0 {
            println!("  No usage data found");
            println!("  Check that you have used {} locally", result.display_name);
        } else {
            // Total cost
            if use_color {
                println!(
                    "  Total:    \x1b[32m{}\x1b[0m",
                    result.summary.format_total()
                );
            } else {
                println!("  Total:    {}", result.summary.format_total());
            }

            // Codex speed tier (ccusage --speed parity) so Reddit/A-B compares
            // can say whether dollars are standard list or priority/fast 2×.
            if result.provider == "codex"
                && let Some(speed) = result.summary.codex_cost_speed.as_deref()
            {
                match result.summary.codex_service_tier.as_deref() {
                    Some(tier) => {
                        println!("  Speed:    {} (config service_tier={})", speed, tier)
                    }
                    None => println!("  Speed:    {}", speed),
                }
            }

            // Token breakdown
            println!(
                "  Tokens:   {} input, {} output, {} cached",
                format_number(result.summary.input_tokens),
                format_number(result.summary.output_tokens),
                format_number(result.summary.cached_tokens)
            );
            if result.summary.cache_read_tokens > 0 || result.summary.cache_write_tokens > 0 {
                println!(
                    "  Cache:    {} read, {} written",
                    format_number(result.summary.cache_read_tokens),
                    format_number(result.summary.cache_write_tokens)
                );
            }

            // Sessions
            println!("  Sessions: {}", result.summary.sessions_count);

            // Cost by model
            if !result.summary.by_model.is_empty() {
                println!("  By model:");
                let mut models: Vec<_> = result.summary.by_model.iter().collect();
                models.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
                for (model, cost) in models {
                    println!("    {}: ${:.2}", model, cost);
                }
            }

            // Render every effort tier that has tokens, including unpriced
            // usage (present in by_effort_tokens but not by_effort). Cost
            // defaults to $0.00, matching the JSON contract.
            if !result.summary.by_effort_tokens.is_empty() {
                println!("  Codex effort:");
                let cost_of =
                    |bucket: &str| result.summary.by_effort.get(bucket).copied().unwrap_or(0.0);
                let mut efforts: Vec<_> = result.summary.by_effort_tokens.iter().collect();
                efforts.sort_by(|(a, _), (b, _)| {
                    cost_of(b)
                        .partial_cmp(&cost_of(a))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                for (bucket, counts) in efforts {
                    println!(
                        "    {}: ${:.2} ({} tokens)",
                        bucket,
                        cost_of(bucket),
                        format_number(counts.total())
                    );
                }
            }
        }

        if i < results.len() - 1 {
            println!();
        }
    }
}

/// Print JSON output
fn print_json_output(results: &[CostResult], pretty: bool, days: u32) -> anyhow::Result<()> {
    let payloads: Vec<serde_json::Value> = results.iter().map(|r| result_json(r, days)).collect();

    let output = if pretty {
        serde_json::to_string_pretty(&payloads)?
    } else {
        serde_json::to_string(&payloads)?
    };
    println!("{}", output);

    Ok(())
}

/// Format a number with commas
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(c);
    }
    result
}

/// Check if stdout is a terminal
fn is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_number_groups_by_thousands() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(7), "7");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
        assert_eq!(format_number(1_234), "1,234");
        assert_eq!(format_number(1_234_567), "1,234,567");
    }

    #[test]
    fn format_number_handles_u64_max() {
        assert_eq!(format_number(u64::MAX), "18,446,744,073,709,551,615");
    }

    /// SBS-934: a default Enabled-providers cost run listed Grok as
    /// unsupported and told the user only Codex and Claude have local logs.
    #[test]
    fn grok_cost_is_supported_and_does_not_claim_codex_claude_only() {
        let scanner = CostScanner::new(7);
        let results = collect_results(&scanner, &[ProviderId::Grok, ProviderId::Cursor]);
        assert!(results[0].supported, "grok must be a local-log scanner");
        assert!(!results[1].supported, "cursor stays unsupported");
        let grok_json = result_json(&results[0], 7);
        assert_eq!(grok_json["supported"], true);
        assert!(json_has_no_error(&grok_json));
        let cursor_json = result_json(&results[1], 7);
        assert_eq!(cursor_json["supported"], false);
        assert!(
            !UNSUPPORTED_LOCAL_LOGS_HINT.contains("Only Codex and Claude have local logs"),
            "hint must name Grok: {UNSUPPORTED_LOCAL_LOGS_HINT}"
        );
        assert!(
            UNSUPPORTED_LOCAL_LOGS_HINT.contains("Grok"),
            "hint must name Grok: {UNSUPPORTED_LOCAL_LOGS_HINT}"
        );
    }

    fn json_has_no_error(value: &serde_json::Value) -> bool {
        value.get("error").is_none()
    }

    /// SBS-934: when Grok sessions exist, cost must report their ticks the
    /// same way Charts already does — not `supported: false`.
    #[test]
    fn grok_cost_reads_session_ticks_when_sessions_exist() {
        let home = tempfile::tempdir().unwrap();
        let session = home
            .path()
            .join("sessions")
            .join("proj")
            .join("019f-session");
        std::fs::create_dir_all(&session).unwrap();
        let now = chrono::Utc::now();
        let ts = now.timestamp() as f64;
        let ms = now.timestamp_millis();
        let updates = format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s1","_meta":{{"eventId":"e1","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"turn_completed","prompt_id":"p1","usage":{{"inputTokens":1000,"outputTokens":100,"cachedReadTokens":800,"reasoningTokens":40,"modelCalls":17,"costUsdTicks":5912850000,"modelUsage":{{"grok-4.5-build":{{"inputTokens":1000,"outputTokens":100,"cachedReadTokens":800,"reasoningTokens":40,"modelCalls":17,"costUsdTicks":5912850000}}}}}}}}}}}}"#
        );
        std::fs::write(session.join("updates.jsonl"), updates).unwrap();
        std::fs::write(
            session.join("summary.json"),
            r#"{"info":{"cwd":"C:\\projects\\personal\\ceiling"},"reasoning_effort":"high","current_model_id":"grok-4.5"}"#,
        )
        .unwrap();

        let scanner = CostScanner::new(7).with_ambient_home(home.path().to_path_buf());
        let results = collect_results(&scanner, &[ProviderId::Grok]);
        assert!(results[0].supported);
        assert_eq!(results[0].summary.sessions_count, 1);
        assert!(
            (results[0].summary.total_cost_usd - 0.591285).abs() < 1e-9,
            "got {}",
            results[0].summary.total_cost_usd
        );
        let payload = result_json(&results[0], 7);
        assert_eq!(payload["supported"], true);
        assert_eq!(payload["sessions_count"], 1);
    }

    /// SBS-934: default enabled providers include grok; that entry must not
    /// be the unsupported arm.
    #[test]
    fn default_enabled_cost_marks_grok_supported() {
        let settings = crate::settings::Settings::default();
        let providers = ProviderSelection::Enabled.as_list_from(&settings);
        assert!(
            providers.contains(&ProviderId::Grok),
            "default enabled set must still include grok: {providers:?}"
        );
        let results = collect_results(&CostScanner::new(1), &providers);
        let grok = results
            .iter()
            .find(|r| r.provider == "grok")
            .expect("grok row");
        assert!(grok.supported);
        let cursor = results
            .iter()
            .find(|r| r.provider == "cursor")
            .expect("cursor row");
        assert!(!cursor.supported);
    }
}
