//! Cross-snapshot enforcement tracking.
//!
//! A window that vanishes from an otherwise-successful provider response is not
//! the same as a window the provider explicitly lifted. The provider adapters
//! already emit the explicit "not currently enforced" case as an inactive
//! window; this tracker adds the missing third state, `unavailable`, by
//! remembering which windows a provider was reporting and flagging any that
//! quietly drop out. Surfaces can then show honest uncertainty instead of
//! silently losing a limit or fabricating a percentage for it.
//!
//! Scope is `provider | data source | account identity` (shared with the
//! capacity-event observer), so accounts, sources, and providers never bleed
//! into each other. State is process-local: the first read of each scope after
//! launch is a fresh baseline that never emits `unavailable`, mirroring the
//! observer's re-baseline-on-launch behaviour so changes that happened while
//! Ceiling was closed are not replayed as surprises.

use std::collections::HashMap;

use crate::capacity_events::{
    ignored_capacity_window, observation_scope, semantic_inactive_window_id, semantic_window_id,
};
use crate::commands::{InactiveRateWindowSnapshot, ProviderUsageSnapshot};

/// Providers Ceiling treats as first-class subscription meters. Only these are
/// tracked: minor or experimental providers have noisier payloads where an
/// omitted window is routine, and flagging those would be more noise than
/// signal. Keep this list small and deliberate.
const FIRST_CLASS_PROVIDERS: &[&str] =
    &["claude", "codex", "cursor", "copilot", "gemini", "factory"];

const UNAVAILABLE_DESCRIPTION: &str = "Not reported in the latest update";

#[derive(Debug, Default)]
pub struct EnforcementTracker {
    /// scope -> (semantic window id -> last-seen title)
    seen_windows: HashMap<String, HashMap<String, String>>,
}

impl EnforcementTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `unavailable` inactive rows for tracked windows that dropped out
    /// of this successful snapshot. Additive only — it never edits or removes a
    /// real window, so it cannot regress metered display or event detection.
    /// Returns the titles of any windows newly flagged unavailable (for logging).
    pub fn annotate(&mut self, snapshot: &mut ProviderUsageSnapshot) -> Vec<String> {
        // An errored snapshot is not a successful response; do not treat a
        // window's absence as meaningful, and do not disturb the baseline.
        if snapshot.error.is_some() {
            return Vec::new();
        }
        if !FIRST_CLASS_PROVIDERS.contains(&snapshot.provider_id.as_str()) {
            return Vec::new();
        }

        let scope = observation_scope(snapshot);
        let present = present_window_identities(snapshot);
        let mut newly_unavailable = Vec::new();

        if let Some(previous) = self.seen_windows.get(&scope) {
            let mut missing: Vec<(&String, &String)> = previous
                .iter()
                .filter(|(id, _)| !present.contains_key(id.as_str()))
                .collect();
            // Deterministic ordering keeps output stable across refreshes.
            missing.sort_by(|a, b| a.0.cmp(b.0));
            for (id, title) in missing {
                if snapshot.inactive_rate_windows.iter().any(|w| &w.id == id) {
                    continue;
                }
                snapshot.inactive_rate_windows.push(InactiveRateWindowSnapshot {
                    id: id.clone(),
                    title: title.clone(),
                    description: UNAVAILABLE_DESCRIPTION.to_string(),
                    state: "unavailable".to_string(),
                });
                newly_unavailable.push(title.clone());
            }
        }

        // Baseline is the genuinely-present set only; the synthetic unavailable
        // rows are deliberately excluded so a window that stays missing keeps
        // being flagged, and a window that returns simply stops being flagged.
        self.seen_windows.insert(scope, present);
        newly_unavailable
    }
}

/// The semantic id -> title of every window the provider is currently
/// reporting, whether metered (primary/secondary) or explicitly inactive.
/// Mirrors the capacity-event observer's identity scheme so a window keeps the
/// same id as it moves between tracked and not-enforced.
fn present_window_identities(snapshot: &ProviderUsageSnapshot) -> HashMap<String, String> {
    let mut out = HashMap::new();

    let primary_label = snapshot.primary_label.as_deref().unwrap_or("Plan");
    out.insert(
        semantic_window_id(primary_label, snapshot.primary.window_minutes),
        primary_label.to_string(),
    );

    if let Some(secondary) = snapshot.secondary.as_ref() {
        let label = snapshot.secondary_label.as_deref().unwrap_or("Secondary");
        out.insert(
            semantic_window_id(label, secondary.window_minutes),
            label.to_string(),
        );
    }

    for extra in &snapshot.extra_rate_windows {
        if ignored_capacity_window(snapshot, &extra.id, &extra.title) {
            continue;
        }
        out.insert(
            semantic_inactive_window_id(&snapshot.provider_id, &extra.id, &extra.title),
            extra.title.clone(),
        );
    }

    for inactive in &snapshot.inactive_rate_windows {
        if ignored_capacity_window(snapshot, &inactive.id, &inactive.title) {
            continue;
        }
        out.insert(
            semantic_inactive_window_id(&snapshot.provider_id, &inactive.id, &inactive.title),
            inactive.title.clone(),
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{NamedRateWindowSnapshot, RateWindowSnapshot};

    fn window(minutes: Option<u32>) -> RateWindowSnapshot {
        RateWindowSnapshot {
            used_percent: 40.0,
            remaining_percent: 60.0,
            window_minutes: minutes,
            resets_at: None,
            reset_description: None,
            is_exhausted: false,
            reserve_percent: None,
            reserve_description: None,
            reserve_will_last_to_reset: false,
            reserve_eta_seconds: None,
        }
    }

    fn codex_snapshot() -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider_id: "codex".into(),
            display_name: "Codex".into(),
            primary: window(Some(300)),
            primary_label: Some("5-hour".into()),
            secondary: Some(window(Some(10_080))),
            secondary_label: Some("Weekly".into()),
            model_specific: None,
            tertiary: None,
            extra_rate_windows: Vec::new(),
            inactive_rate_windows: Vec::new(),
            promo_signals: Vec::new(),
            reset_credits_available: None,
            cost: None,
            plan_name: None,
            account_email: Some("person@example.com".into()),
            source_label: "oauth".into(),
            updated_at: "2026-07-17T00:00:00Z".into(),
            error: None,
            pace: None,
            account_organization: None,
            tray_status_label: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
        }
    }

    fn unavailable_titles(snapshot: &ProviderUsageSnapshot) -> Vec<String> {
        snapshot
            .inactive_rate_windows
            .iter()
            .filter(|w| w.state == "unavailable")
            .map(|w| w.title.clone())
            .collect()
    }

    #[test]
    fn first_read_is_a_baseline_and_never_flags_absence() {
        let mut tracker = EnforcementTracker::new();
        let mut only_five_hour = codex_snapshot();
        only_five_hour.secondary = None;
        only_five_hour.secondary_label = None;

        assert!(tracker.annotate(&mut only_five_hour).is_empty());
        assert!(only_five_hour.inactive_rate_windows.is_empty());
    }

    #[test]
    fn a_window_that_drops_out_becomes_unavailable() {
        let mut tracker = EnforcementTracker::new();
        // Baseline: both 5-hour and weekly present.
        tracker.annotate(&mut codex_snapshot());

        // Next read omits the weekly window entirely.
        let mut dropped = codex_snapshot();
        dropped.secondary = None;
        dropped.secondary_label = None;

        let flagged = tracker.annotate(&mut dropped);
        assert_eq!(flagged, vec!["Weekly".to_string()]);
        assert_eq!(unavailable_titles(&dropped), vec!["Weekly".to_string()]);
        assert_eq!(dropped.inactive_rate_windows[0].state, "unavailable");
    }

    #[test]
    fn an_explicitly_lifted_window_is_not_unavailable() {
        let mut tracker = EnforcementTracker::new();
        tracker.annotate(&mut codex_snapshot());

        // Weekly is reported as inactive (provider lifted it), not absent.
        let mut lifted = codex_snapshot();
        lifted.secondary = None;
        lifted.secondary_label = None;
        lifted.inactive_rate_windows.push(InactiveRateWindowSnapshot {
            id: "codex-weekly".into(),
            title: "Weekly".into(),
            description: "Not currently enforced".into(),
            state: "notEnforced".into(),
        });

        assert!(tracker.annotate(&mut lifted).is_empty());
        assert!(unavailable_titles(&lifted).is_empty());
    }

    #[test]
    fn a_returning_window_stops_being_flagged() {
        let mut tracker = EnforcementTracker::new();
        tracker.annotate(&mut codex_snapshot());

        let mut dropped = codex_snapshot();
        dropped.secondary = None;
        dropped.secondary_label = None;
        assert_eq!(tracker.annotate(&mut dropped).len(), 1);

        // Weekly comes back on the following read.
        let mut restored = codex_snapshot();
        assert!(tracker.annotate(&mut restored).is_empty());
        assert!(unavailable_titles(&restored).is_empty());
    }

    #[test]
    fn errored_snapshot_is_ignored_and_does_not_disturb_the_baseline() {
        let mut tracker = EnforcementTracker::new();
        tracker.annotate(&mut codex_snapshot());

        let mut errored = codex_snapshot();
        errored.secondary = None;
        errored.error = Some("network".into());
        assert!(tracker.annotate(&mut errored).is_empty());
        assert!(errored.inactive_rate_windows.is_empty());

        // The failed read must not have consumed the baseline: a later
        // successful read that still omits weekly is flagged.
        let mut dropped = codex_snapshot();
        dropped.secondary = None;
        dropped.secondary_label = None;
        assert_eq!(tracker.annotate(&mut dropped), vec!["Weekly".to_string()]);
    }

    #[test]
    fn non_first_class_providers_are_not_tracked() {
        let mut tracker = EnforcementTracker::new();
        let mut first = codex_snapshot();
        first.provider_id = "venice".into();
        tracker.annotate(&mut first);

        let mut dropped = codex_snapshot();
        dropped.provider_id = "venice".into();
        dropped.secondary = None;
        dropped.secondary_label = None;
        assert!(tracker.annotate(&mut dropped).is_empty());
        assert!(dropped.inactive_rate_windows.is_empty());
    }

    #[test]
    fn different_accounts_do_not_flag_each_other() {
        let mut tracker = EnforcementTracker::new();
        tracker.annotate(&mut codex_snapshot());

        // A different account's first read is its own baseline, even though it
        // omits weekly relative to the first account.
        let mut other = codex_snapshot();
        other.account_email = Some("other@example.com".into());
        other.secondary = None;
        other.secondary_label = None;
        assert!(tracker.annotate(&mut other).is_empty());
        assert!(other.inactive_rate_windows.is_empty());
    }

    #[test]
    fn extra_named_windows_are_tracked_too() {
        let mut tracker = EnforcementTracker::new();
        let mut with_spark = codex_snapshot();
        with_spark.extra_rate_windows.push(NamedRateWindowSnapshot {
            id: "codex-spark-weekly".into(),
            title: "Codex Spark Weekly".into(),
            window: window(Some(10_080)),
        });
        tracker.annotate(&mut with_spark);

        // Spark disappears on the next successful read.
        let mut dropped = codex_snapshot();
        let flagged = tracker.annotate(&mut dropped);
        assert_eq!(flagged, vec!["Codex Spark Weekly".to_string()]);
    }
}
