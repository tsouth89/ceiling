use serde::Deserialize;

use super::UtilizationScale;
use crate::core::{CostSnapshot, NamedRateWindow, RateWindow, UsageSnapshot};

/// Usage payload shared by Claude's OAuth and web endpoints.
///
/// The two endpoints expose the same windows with different casing and have
/// drifted independently in the past. Normalize the wire shape once so every
/// source renders the same set of windows and extra-usage dollars.
#[derive(Debug)]
pub struct ClaudeUsageResponse {
    pub five_hour: Option<ClaudeUsageWindow>,
    pub seven_day: Option<ClaudeUsageWindow>,
    pub seven_day_opus: Option<ClaudeUsageWindow>,
    pub seven_day_sonnet: Option<ClaudeUsageWindow>,
    pub seven_day_oauth_apps: Option<ClaudeUsageWindow>,
    pub seven_day_design: Option<ClaudeUsageWindow>,
    pub seven_day_promotional: Option<ClaudeUsageWindow>,
    pub seven_day_routines: Option<ClaudeUsageWindow>,
    pub extra_usage: Option<ClaudeExtraUsage>,
    pub(super) limits: Vec<super::scoped_weekly::ScopedWeeklyLimit>,
}

impl<'de> Deserialize<'de> for ClaudeUsageResponse {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::deserialize(deserializer)?;

        let take_window = |map: &mut std::collections::HashMap<String, serde_json::Value>,
                           keys: &[&str]|
         -> Result<Option<ClaudeUsageWindow>, D::Error> {
            for key in keys {
                if let Some(value) = map.remove(*key) {
                    if value.is_null() {
                        continue;
                    }
                    return serde_json::from_value(value)
                        .map(Some)
                        .map_err(serde::de::Error::custom);
                }
            }
            Ok(None)
        };

        let take_value = |map: &mut std::collections::HashMap<String, serde_json::Value>,
                          keys: &[&str]| {
            keys.iter()
                .find_map(|key| map.remove(*key).filter(|value| !value.is_null()))
        };

        Ok(Self {
            five_hour: take_window(&mut map, &["five_hour", "fiveHour"])?,
            seven_day: take_window(&mut map, &["seven_day", "sevenDay"])?,
            seven_day_opus: take_window(&mut map, &["seven_day_opus", "sevenDayOpus"])?,
            seven_day_sonnet: take_window(&mut map, &["seven_day_sonnet", "sevenDaySonnet"])?,
            seven_day_oauth_apps: take_window(
                &mut map,
                &[
                    "seven_day_oauth_apps",
                    "sevenDayOAuthApps",
                    "seven_day_claude_oauth_apps",
                    "oauth_apps",
                    "oauth",
                ],
            )?,
            seven_day_design: take_window(
                &mut map,
                &[
                    "seven_day_design",
                    "sevenDayDesign",
                    "seven_day_claude_design",
                    "claude_design",
                    "design",
                ],
            )?,
            seven_day_promotional: take_window(
                &mut map,
                &[
                    "omelette_promotional",
                    "omelettePromotional",
                    "omelette",
                    "seven_day_omelette",
                    "sevenDayOmelette",
                ],
            )?,
            seven_day_routines: take_window(
                &mut map,
                &[
                    "seven_day_routines",
                    "sevenDayRoutines",
                    "seven_day_claude_routines",
                    "claude_routines",
                    "routines",
                    "routine",
                    "seven_day_cowork",
                    "sevenDayCowork",
                    "cowork",
                ],
            )?,
            limits: take_value(&mut map, &["limits"])
                .map(serde_json::from_value)
                .transpose()
                .map_err(serde::de::Error::custom)?
                .unwrap_or_default(),
            extra_usage: take_value(&mut map, &["extra_usage", "extraUsage"])
                .map(serde_json::from_value)
                .transpose()
                .map_err(serde::de::Error::custom)?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct ClaudeUsageWindow {
    pub utilization: Option<f64>,
    #[serde(rename = "resets_at", alias = "resetsAt")]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeExtraUsage {
    #[serde(
        rename = "monthly_credit_limit",
        alias = "monthlyCreditLimit",
        alias = "monthly_limit",
        alias = "monthlyLimit"
    )]
    pub monthly_limit: Option<f64>,
    #[serde(rename = "used_credits", alias = "usedCredits")]
    pub used_credits: Option<f64>,
    pub currency: Option<String>,
    #[serde(rename = "is_enabled", alias = "isEnabled")]
    pub is_enabled: Option<bool>,
}

impl ClaudeUsageResponse {
    pub(super) fn utilization_scale(&self) -> UtilizationScale {
        UtilizationScale::detect(
            [
                self.five_hour.as_ref(),
                self.seven_day.as_ref(),
                self.seven_day_opus.as_ref(),
                self.seven_day_sonnet.as_ref(),
                self.seven_day_oauth_apps.as_ref(),
                self.seven_day_design.as_ref(),
                self.seven_day_promotional.as_ref(),
                self.seven_day_routines.as_ref(),
            ]
            .into_iter()
            .flatten()
            .filter_map(|window| window.utilization),
        )
    }

    pub(super) fn build_snapshot<F>(&self, mut convert: F) -> UsageSnapshot
    where
        F: FnMut(&ClaudeUsageWindow, Option<u32>, UtilizationScale) -> Option<RateWindow>,
    {
        let scale = self.utilization_scale();
        let primary = self
            .five_hour
            .as_ref()
            .and_then(|window| convert(window, Some(300), scale))
            .unwrap_or_else(|| RateWindow::new(0.0));
        let mut snapshot = UsageSnapshot::new(primary);

        if let Some(window) = self
            .seven_day
            .as_ref()
            .and_then(|window| convert(window, Some(10080), scale))
        {
            snapshot = snapshot.with_secondary(window);
        }

        let model_specific = self
            .seven_day_opus
            .as_ref()
            .and_then(|window| convert(window, Some(10080), scale))
            .or_else(|| {
                self.seven_day_sonnet
                    .as_ref()
                    .and_then(|window| convert(window, Some(10080), scale))
            });
        if let Some(window) = model_specific {
            snapshot = snapshot.with_model_specific(window);
        }

        for (id, title, window) in [
            (
                "claude-oauth-apps",
                "OAuth apps",
                self.seven_day_oauth_apps.as_ref(),
            ),
            (
                "claude-routines",
                "Daily Routines",
                self.seven_day_routines.as_ref(),
            ),
            ("claude-design", "Design", self.seven_day_design.as_ref()),
            (
                "claude-weekly-promo",
                "Weekly promo",
                self.seven_day_promotional.as_ref(),
            ),
        ] {
            if let Some(window) = window.and_then(|window| convert(window, Some(10080), scale)) {
                snapshot
                    .extra_rate_windows
                    .push(NamedRateWindow::new(id, title, window));
            }
        }

        snapshot
            .extra_rate_windows
            .extend(super::scoped_weekly::scoped_weekly_windows(&self.limits));
        snapshot
    }

    pub(super) fn extra_usage_cost(&self) -> Option<CostSnapshot> {
        self.extra_usage.as_ref()?.cost_snapshot()
    }
}

impl ClaudeExtraUsage {
    pub(super) fn cost_snapshot(&self) -> Option<CostSnapshot> {
        let extra = self;
        if !extra.is_enabled.unwrap_or(false) {
            return None;
        }

        let mut cost = CostSnapshot::new(
            extra.used_credits.unwrap_or(0.0) / 100.0,
            extra.currency.clone().unwrap_or_else(|| "USD".to_string()),
            "Monthly",
        );
        if let Some(limit) = extra.monthly_limit {
            cost = cost.with_limit(limit / 100.0);
        }
        Some(cost)
    }
}

#[cfg(test)]
mod tests {
    use super::ClaudeUsageResponse;
    use crate::core::RateWindow;

    fn snapshot(response: &ClaudeUsageResponse) -> crate::core::UsageSnapshot {
        response.build_snapshot(|window, minutes, scale| {
            Some(RateWindow::with_details(
                scale.to_percent(window.utilization?),
                minutes,
                None,
                None,
            ))
        })
    }

    #[test]
    fn oauth_shape_keeps_design_and_extra_usage_dollars() {
        let response: ClaudeUsageResponse = serde_json::from_str(
            r#"{
                "fiveHour": {"utilization": 12},
                "sevenDayDesign": {"utilization": 34},
                "extraUsage": {
                    "isEnabled": true,
                    "usedCredits": 1234,
                    "monthlyLimit": 5000,
                    "currency": "USD"
                }
            }"#,
        )
        .expect("OAuth response parses");

        let usage = snapshot(&response);
        let design = usage
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "claude-design")
            .expect("Design window survives OAuth mapping");
        assert_eq!(design.window.used_percent, 34.0);

        let cost = response.extra_usage_cost().expect("extra usage cost");
        assert_eq!(cost.used, 12.34);
        assert_eq!(cost.limit, Some(50.0));
        assert_eq!(cost.currency_code, "USD");
    }

    #[test]
    fn web_shape_keeps_sonnet_as_the_model_specific_window() {
        let response: ClaudeUsageResponse = serde_json::from_str(
            r#"{
                "five_hour": {"utilization": 12},
                "seven_day": {"utilization": 23},
                "seven_day_sonnet": {"utilization": 45}
            }"#,
        )
        .expect("web response parses");

        let usage = snapshot(&response);
        assert_eq!(
            usage
                .model_specific
                .expect("Sonnet model-specific window")
                .used_percent,
            45.0
        );
    }
}
