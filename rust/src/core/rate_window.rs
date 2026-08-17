//! Rate window model - represents a usage limit window (e.g., 5-hour session, 7-day weekly)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Represents a rate limit window with usage percentage and reset time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateWindow {
    /// Percentage of the window that has been used (0-100)
    pub used_percent: f64,

    /// Duration of the window in minutes (e.g., 300 for 5-hour, 10080 for 7-day)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,

    /// When the window resets
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,

    /// Human-readable reset description (e.g., "Jan 15 at 3:00pm")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_description: Option<String>,
}

/// Compact remaining-time string used by every reset-countdown surface.
///
/// Floor one total of minutes, clamp a still-future sub-minute remainder
/// to 1 so the last 59s never read as "0m", and cut days at 1440 minutes
/// so 24h 0m is "1d 0h" rather than "24h 0m". Matches the TypeScript
/// hooks and `tooltip_short_reset`. SBS-927 (day cut and last minute)
/// and SBS-619 (hour-boundary floor).
pub fn remaining_countdown_parts(remaining_seconds: i64) -> (i64, i64, i64) {
    let total_minutes = if remaining_seconds > 0 {
        (remaining_seconds / 60).max(1)
    } else {
        0
    };
    (
        total_minutes / 1440,
        (total_minutes % 1440) / 60,
        total_minutes % 60,
    )
}

/// Compact `{d}d {h}h` / `{h}h {m}m` / `{m}m` form of [`remaining_countdown_parts`].
pub fn format_remaining_countdown(remaining_seconds: i64) -> String {
    let (days, hours, minutes) = remaining_countdown_parts(remaining_seconds);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

impl RateWindow {
    /// Create a new rate window
    pub fn new(used_percent: f64) -> Self {
        Self {
            used_percent: Self::finite_percent(used_percent),
            window_minutes: None,
            resets_at: None,
            reset_description: None,
        }
    }

    /// Create a rate window with full details
    pub fn with_details(
        used_percent: f64,
        window_minutes: Option<u32>,
        resets_at: Option<DateTime<Utc>>,
        reset_description: Option<String>,
    ) -> Self {
        Self {
            used_percent: Self::finite_percent(used_percent),
            window_minutes,
            resets_at,
            reset_description,
        }
    }

    /// Get the remaining percentage (100 - used)
    pub fn remaining_percent(&self) -> f64 {
        100.0 - self.used_percent
    }

    /// Check if the window is exhausted (>= 100% used)
    pub fn is_exhausted(&self) -> bool {
        self.used_percent >= 100.0
    }

    /// Check if the window is nearly exhausted (>= 90% used)
    pub fn is_nearly_exhausted(&self) -> bool {
        self.used_percent >= 90.0
    }

    /// Format the reset time as a countdown string
    pub fn format_countdown(&self) -> Option<String> {
        self.format_countdown_at(Utc::now())
    }

    /// `format_countdown` against an explicit clock.
    ///
    /// Exists so the rounding can be asserted deterministically: reading
    /// `Utc::now()` inside meant a test building `now + 42min` measured
    /// marginally less than that by the time it was formatted, and flooring
    /// turned those microseconds into a whole minute of difference.
    pub(crate) fn format_countdown_at(&self, now: DateTime<Utc>) -> Option<String> {
        let resets_at = self.resets_at?;

        if resets_at <= now {
            return Some("now".to_string());
        }

        Some(format_remaining_countdown((resets_at - now).num_seconds()))
    }

    fn finite_percent(value: f64) -> f64 {
        if value.is_finite() {
            value.clamp(0.0, 100.0)
        } else {
            0.0
        }
    }
}

impl Default for RateWindow {
    fn default() -> Self {
        Self::new(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remaining_percent() {
        let window = RateWindow::new(75.0);
        assert!((window.remaining_percent() - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_clamping() {
        let window = RateWindow::new(150.0);
        assert!((window.used_percent - 100.0).abs() < f64::EPSILON);

        let window = RateWindow::new(-10.0);
        assert!(window.used_percent.abs() < f64::EPSILON);
    }

    #[test]
    fn test_exhausted() {
        assert!(RateWindow::new(100.0).is_exhausted());
        assert!(!RateWindow::new(99.0).is_exhausted());
    }

    #[test]
    fn countdown_uses_one_minute_for_sub_minute_future_reset() {
        let window = RateWindow::with_details(
            10.0,
            None,
            Some(Utc::now() + chrono::Duration::seconds(30)),
            None,
        );

        assert_eq!(window.format_countdown().as_deref(), Some("1m"));
    }

    /// SBS-619: hours came from a floor and minutes from a ceil, so just under
    /// an hour boundary the minutes wrapped to 0 while the hours had not yet
    /// advanced — reporting nearly an hour less time than remained.
    #[test]
    fn countdown_does_not_lose_an_hour_at_a_boundary() {
        let now = "2026-04-02T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let countdown = |seconds: i64| {
            RateWindow::with_details(
                10.0,
                None,
                Some(now + chrono::Duration::seconds(seconds)),
                None,
            )
            .format_countdown_at(now)
        };

        // 1h59m30s floors to 119 minutes. Never "1h 0m".
        assert_eq!(countdown(2 * 3600 - 30).as_deref(), Some("1h 59m"));
        assert_eq!(countdown(2 * 3600 - 90).as_deref(), Some("1h 58m"));
        // Ordinary readings are unchanged.
        assert_eq!(countdown(3600 + 20 * 60).as_deref(), Some("1h 20m"));
        assert_eq!(countdown(42 * 60).as_deref(), Some("42m"));
    }

    /// The taskbar tile (Rust) and the tray flyout (TypeScript) render the same
    /// `resets_at`, so they must round identically. Mirrors the hooks'
    /// `Math.max(1, Math.floor(diffMs / 60_000))`.
    #[test]
    fn countdown_rounds_the_same_way_the_typescript_hooks_do() {
        let now = "2026-04-02T12:00:00Z".parse::<DateTime<Utc>>().unwrap();

        for (seconds, expected) in [
            (45_i64, "1m"),
            (61, "1m"),
            (119, "1m"),
            (42 * 60, "42m"),
            (2 * 3600 - 30, "1h 59m"),
            (3600 + 20 * 60, "1h 20m"),
            // SBS-927: the day cut is minutes/1440, not hours > 24.
            (24 * 3600, "1d 0h"),
            (24 * 3600 + 1, "1d 0h"),
            (24 * 3600 + 30 * 60, "1d 0h"),
            (25 * 3600, "1d 1h"),
            (23 * 3600 + 59 * 60 + 59, "23h 59m"),
        ] {
            let actual = RateWindow::with_details(
                10.0,
                None,
                Some(now + chrono::Duration::seconds(seconds)),
                None,
            )
            .format_countdown_at(now);
            assert_eq!(actual.as_deref(), Some(expected), "at {seconds}s");
        }
    }

    /// SBS-927: CLI used `hours > 24`, so a remaining day stayed "24h Xm"
    /// while the tray and the TypeScript hooks already rendered "1d 0h".
    #[test]
    fn countdown_cuts_a_day_at_twenty_four_hours() {
        let now = "2026-04-02T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let countdown = |seconds: i64| {
            RateWindow::with_details(
                10.0,
                None,
                Some(now + chrono::Duration::seconds(seconds)),
                None,
            )
            .format_countdown_at(now)
        };

        assert_eq!(countdown(24 * 3600).as_deref(), Some("1d 0h"));
        assert_eq!(countdown(24 * 3600 + 1).as_deref(), Some("1d 0h"));
        assert_eq!(countdown(24 * 3600 + 10 * 60).as_deref(), Some("1d 0h"));
        assert_eq!(countdown(24 * 3600 + 30 * 60).as_deref(), Some("1d 0h"));
        assert_eq!(countdown(25 * 3600).as_deref(), Some("1d 1h"));
        assert_eq!(countdown(30).as_deref(), Some("1m"));
    }

    /// The shared helper is what CLI, tooltip, Codex, pace, and Z.ai call.
    /// Pin both edges here so a later rewrite cannot split them again.
    #[test]
    fn remaining_countdown_agrees_at_the_day_cut_and_last_minute() {
        assert_eq!(format_remaining_countdown(30), "1m");
        assert_eq!(format_remaining_countdown(59), "1m");
        assert_eq!(format_remaining_countdown(60), "1m");
        assert_eq!(format_remaining_countdown(24 * 3600), "1d 0h");
        assert_eq!(format_remaining_countdown(24 * 3600 + 1), "1d 0h");
        assert_eq!(format_remaining_countdown(24 * 3600 + 30 * 60), "1d 0h");
        assert_eq!(format_remaining_countdown(25 * 3600), "1d 1h");
        assert_eq!(format_remaining_countdown(0), "0m");
    }
}
