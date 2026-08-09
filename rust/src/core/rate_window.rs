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
        let resets_at = self.resets_at?;
        let now = Utc::now();

        if resets_at <= now {
            return Some("now".to_string());
        }

        let duration = resets_at - now;
        // Derive hours and minutes from ONE rounded total. Taking hours from
        // `num_hours()` (floor) while taking minutes from a ceiled total let
        // the two disagree across an hour boundary: at 1h59m30s the total
        // rounded up into the next hour, so minutes read 0 while hours stayed
        // 1 and the countdown claimed "1h 0m" — nearly an hour short.
        let total_minutes = ((duration.num_seconds() + 59) / 60).max(1);
        let hours = total_minutes / 60;
        let minutes = total_minutes % 60;

        if hours > 24 {
            let days = hours / 24;
            Some(format!("{}d {}h", days, hours % 24))
        } else if hours > 0 {
            Some(format!("{}h {}m", hours, minutes))
        } else {
            Some(format!("{}m", minutes))
        }
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
        let countdown = |seconds: i64| {
            RateWindow::with_details(
                10.0,
                None,
                Some(Utc::now() + chrono::Duration::seconds(seconds)),
                None,
            )
            .format_countdown()
        };

        // 1h59m30s rounds to 120 minutes: two hours, not "1h 0m".
        assert_eq!(countdown(2 * 3600 - 30).as_deref(), Some("2h 0m"));
        // A minute earlier is still inside the first hour.
        assert_eq!(countdown(2 * 3600 - 90).as_deref(), Some("1h 59m"));
        // Ordinary readings are unchanged.
        assert_eq!(countdown(3600 + 20 * 60).as_deref(), Some("1h 20m"));
        assert_eq!(countdown(42 * 60).as_deref(), Some("42m"));
    }
}
