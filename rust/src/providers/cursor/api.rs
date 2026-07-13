//! Cursor API client for fetching usage information
//!
//! Uses browser cookies to authenticate with cursor.com API

use crate::core::{
    CostSnapshot, InactiveRateWindow, NamedRateWindow, PromoSignal, ProviderError, RateWindow,
    UsageSnapshot,
};
use crate::providers::browser_cookie_header;
use chrono::{DateTime, Utc};
use serde::Deserialize;

const BASE_URL: &str = "https://cursor.com";
const COOKIE_DOMAINS: [&str; 2] = ["cursor.com", "cursor.sh"];
const NOT_ENFORCED: &str = "Not currently enforced by Cursor";

pub(super) type CursorUsageResult = (UsageSnapshot, Option<CostSnapshot>);

/// Cursor API client
pub struct CursorApi {
    client: reqwest::Client,
}

impl CursorApi {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Fetch usage information from Cursor API
    pub async fn fetch_usage(&self) -> Result<CursorUsageResult, ProviderError> {
        let cookie_header = self.get_cookie_header()?;
        self.fetch_usage_with_cookie_header(&cookie_header).await
    }

    /// Fetch usage information with an already resolved Cookie header.
    pub async fn fetch_usage_with_cookie_header(
        &self,
        cookie_header: &str,
    ) -> Result<CursorUsageResult, ProviderError> {
        let (usage_result, user_result) = tokio::join!(
            self.fetch_usage_summary(cookie_header),
            self.fetch_user_info(cookie_header)
        );

        let usage_summary = usage_result?;
        let user_info = user_result.ok();

        self.build_result(usage_summary, user_info)
    }

    pub(super) fn get_cookie_header(&self) -> Result<String, ProviderError> {
        browser_cookie_header(&COOKIE_DOMAINS)
    }

    async fn fetch_usage_summary(
        &self,
        cookie_header: &str,
    ) -> Result<UsageSummary, ProviderError> {
        let url = format!("{}/api/usage-summary", BASE_URL);

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;

        if response.status() == 401 || response.status() == 403 {
            return Err(ProviderError::AuthRequired);
        }

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Cursor API returned {}",
                response.status()
            )));
        }

        let text = response
            .text()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        serde_json::from_str::<UsageSummary>(&text).map_err(|e| {
            tracing::warn!(
                "Cursor usage-summary parse error: {e}; response length: {} bytes",
                text.len()
            );
            ProviderError::Parse(e.to_string())
        })
    }

    async fn fetch_user_info(&self, cookie_header: &str) -> Result<UserInfo, ProviderError> {
        let url = format!("{}/api/auth/me", BASE_URL);

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ProviderError::Other(
                "Failed to fetch user info".to_string(),
            ));
        }

        response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }

    fn build_result(
        &self,
        summary: UsageSummary,
        user_info: Option<UserInfo>,
    ) -> Result<CursorUsageResult, ProviderError> {
        let billing_end = summary
            .billing_cycle_end
            .as_ref()
            .and_then(|s| parse_iso_date(s));
        let window_minutes = monthly_window_minutes(
            summary.billing_cycle_start.as_deref(),
            summary.billing_cycle_end.as_deref(),
        );

        let mut extras = Vec::new();
        let mut inactives = Vec::new();
        let mut secondary = None;
        let mut cost_snapshot = None;
        let mut monthly_percent = None;

        if let Some(individual) = &summary.individual_usage {
            if let Some(plan) = &individual.plan {
                monthly_percent = plan_monthly_percent(plan);

                if let Some(auto) = plan.auto_percent_used {
                    secondary = Some(RateWindow::with_details(
                        auto,
                        window_minutes,
                        billing_end,
                        None,
                    ));
                }

                if let Some(api) = plan.api_percent_used {
                    extras.push(NamedRateWindow::new(
                        "cursor-api",
                        "API",
                        RateWindow::with_details(api, window_minutes, billing_end, None),
                    ));
                }

                if let Some(promo) = promotional_window(plan, window_minutes, billing_end) {
                    extras.push(promo);
                }

                // When Cursor reports the modern percent-lane payload, omitted Auto/API
                // lanes are explicit absences — not fabricated 0% meters.
                if uses_percent_lanes(plan) {
                    if plan.auto_percent_used.is_none() {
                        inactives.push(InactiveRateWindow::new(
                            "cursor-auto",
                            "Auto",
                            NOT_ENFORCED,
                        ));
                    }
                    if plan.api_percent_used.is_none() {
                        inactives.push(InactiveRateWindow::new("cursor-api", "API", NOT_ENFORCED));
                    }
                }

                let on_demand = individual.on_demand.as_ref().or_else(|| {
                    summary
                        .team_usage
                        .as_ref()
                        .and_then(|t| t.on_demand.as_ref())
                });

                if let Some(window) = on_demand_window(on_demand, window_minutes, billing_end) {
                    extras.push(window);
                }

                cost_snapshot = Self::on_demand_cost(on_demand, billing_end)
                    .or_else(|| plan_cost_snapshot(plan, billing_end));
            } else if let Some(overall) = &individual.overall {
                // Overall is a single reported pool — keep it as monthly + cost only.
                monthly_percent = Self::usage_percent(overall);
                cost_snapshot = Self::on_demand_cost(Some(overall), billing_end);
            }
        } else if let Some(team) = &summary.team_usage {
            if let Some(pooled) = &team.pooled {
                monthly_percent = Self::usage_percent(pooled);
                cost_snapshot = Self::on_demand_cost(Some(pooled), billing_end);
            }
            if let Some(window) =
                on_demand_window(team.on_demand.as_ref(), window_minutes, billing_end)
            {
                extras.push(window);
            }
            if cost_snapshot.is_none() {
                cost_snapshot = Self::on_demand_cost(team.on_demand.as_ref(), billing_end);
            }
        }

        let has_monthly_meter = monthly_percent.is_some();
        if summary.is_unlimited == Some(true) && !has_monthly_meter {
            inactives.push(InactiveRateWindow::new(
                "cursor-monthly",
                "Monthly",
                NOT_ENFORCED,
            ));
        }

        let primary = RateWindow::with_details(
            monthly_percent.unwrap_or(0.0),
            window_minutes,
            billing_end,
            None,
        );

        let mut usage = UsageSnapshot::new(primary);
        if let Some(sec) = secondary {
            usage = usage.with_secondary(sec);
        }
        usage.extra_rate_windows = extras;
        usage.inactive_rate_windows = inactives;

        if let Some(promo) = usage
            .extra_rate_windows
            .iter()
            .find(|w| w.id == "cursor-promotional")
        {
            let ends_at = promo.window.resets_at;
            usage = usage.with_promo_signal(PromoSignal::boost(
                "cursor-promotional",
                "Promotional",
                "Bonus promotional capacity reported by Cursor",
                Some("cursor-promotional".to_string()),
                ends_at,
            ));
        }

        if let Some(email) = user_info.as_ref().and_then(|u| u.email.clone()) {
            usage = usage.with_email(email);
        }
        if let Some(plan_type) = membership_label(summary.membership_type.as_deref()) {
            usage = usage.with_login_method(plan_type);
        }

        Ok((usage, cost_snapshot))
    }

    fn on_demand_cost(
        on_demand: Option<&OnDemandUsage>,
        billing_end: Option<DateTime<Utc>>,
    ) -> Option<CostSnapshot> {
        let usage = on_demand?;
        if usage.enabled == Some(false) {
            return None;
        }

        let used_cents = usage.used.unwrap_or(0) as f64;
        let limit_cents = usage
            .limit
            .or_else(|| {
                usage
                    .remaining
                    .map(|remaining| remaining + usage.used.unwrap_or(0))
            })
            .unwrap_or(0) as f64;

        if used_cents <= 0.0 && limit_cents <= 0.0 {
            return None;
        }

        let mut cost = CostSnapshot::new(used_cents / 100.0, "USD", "Monthly");
        if limit_cents > 0.0 {
            cost = cost.with_limit(limit_cents / 100.0);
        }
        if let Some(reset) = billing_end {
            cost = cost.with_resets_at(reset);
        }
        Some(cost)
    }

    fn usage_percent(usage: &OnDemandUsage) -> Option<f64> {
        let used = usage.used.unwrap_or(0) as f64;
        let limit = usage
            .limit
            .or_else(|| {
                usage
                    .remaining
                    .map(|remaining| remaining + usage.used.unwrap_or(0))
            })
            .unwrap_or(0) as f64;
        (limit > 0.0).then_some(used / limit * 100.0)
    }
}

impl Default for CursorApi {
    fn default() -> Self {
        Self::new()
    }
}

// --- API Response Types ---

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSummary {
    billing_cycle_start: Option<String>,
    billing_cycle_end: Option<String>,
    membership_type: Option<String>,
    #[allow(dead_code)]
    limit_type: Option<String>,
    is_unlimited: Option<bool>,
    individual_usage: Option<IndividualUsage>,
    team_usage: Option<TeamUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndividualUsage {
    plan: Option<PlanUsage>,
    on_demand: Option<OnDemandUsage>,
    overall: Option<OnDemandUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanUsage {
    #[allow(dead_code)]
    enabled: Option<bool>,
    used: Option<i64>,
    limit: Option<i64>,
    #[allow(dead_code)]
    remaining: Option<i64>,
    breakdown: Option<PlanBreakdown>,
    auto_percent_used: Option<f64>,
    api_percent_used: Option<f64>,
    total_percent_used: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanBreakdown {
    included: Option<i64>,
    bonus: Option<i64>,
    total: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnDemandUsage {
    enabled: Option<bool>,
    used: Option<i64>,
    limit: Option<i64>,
    remaining: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamUsage {
    on_demand: Option<OnDemandUsage>,
    pooled: Option<OnDemandUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserInfo {
    email: Option<String>,
    #[allow(dead_code)]
    email_verified: Option<bool>,
    #[allow(dead_code)]
    name: Option<String>,
    #[allow(dead_code)]
    sub: Option<String>,
    #[allow(dead_code)]
    created_at: Option<String>,
    #[allow(dead_code)]
    updated_at: Option<String>,
    #[allow(dead_code)]
    picture: Option<String>,
}

// --- Helper functions ---

fn plan_monthly_percent(plan: &PlanUsage) -> Option<f64> {
    if let Some(percent) = plan.total_percent_used {
        return Some(percent);
    }

    let used_cents = plan.used.unwrap_or(0) as f64;
    let limit_cents = plan
        .limit
        .or_else(|| plan.breakdown.as_ref().and_then(|b| b.total))
        .unwrap_or(0) as f64;
    (limit_cents > 0.0).then_some((used_cents / limit_cents) * 100.0)
}

fn uses_percent_lanes(plan: &PlanUsage) -> bool {
    plan.total_percent_used.is_some()
        || plan.auto_percent_used.is_some()
        || plan.api_percent_used.is_some()
}

fn promotional_window(
    plan: &PlanUsage,
    window_minutes: Option<u32>,
    billing_end: Option<DateTime<Utc>>,
) -> Option<NamedRateWindow> {
    let breakdown = plan.breakdown.as_ref()?;
    let bonus = breakdown.bonus.filter(|&b| b > 0)? as f64;
    let included = breakdown.included.filter(|&v| v >= 0)? as f64;
    let used = plan.used.filter(|&v| v >= 0)? as f64;
    let promo_used = (used - included).clamp(0.0, bonus);
    let percent = promo_used / bonus * 100.0;

    Some(NamedRateWindow::new(
        "cursor-promotional",
        "Promotional",
        RateWindow::with_details(percent, window_minutes, billing_end, None),
    ))
}

fn on_demand_window(
    on_demand: Option<&OnDemandUsage>,
    window_minutes: Option<u32>,
    billing_end: Option<DateTime<Utc>>,
) -> Option<NamedRateWindow> {
    let usage = on_demand?;
    if usage.enabled == Some(false) {
        return None;
    }
    let percent = CursorApi::usage_percent(usage)?;
    Some(NamedRateWindow::new(
        "cursor-on-demand",
        "On-demand",
        RateWindow::with_details(percent, window_minutes, billing_end, None),
    ))
}

fn plan_cost_snapshot(
    plan: &PlanUsage,
    billing_end: Option<DateTime<Utc>>,
) -> Option<CostSnapshot> {
    let used_cents = plan.used? as f64;
    let limit_cents = plan
        .limit
        .or_else(|| plan.breakdown.as_ref().and_then(|b| b.total))
        .unwrap_or(0) as f64;

    if used_cents <= 0.0 && limit_cents <= 0.0 {
        return None;
    }

    let mut cost = CostSnapshot::new(used_cents / 100.0, "USD", "Monthly");
    if limit_cents > 0.0 {
        cost = cost.with_limit(limit_cents / 100.0);
    }
    if let Some(reset) = billing_end {
        cost = cost.with_resets_at(reset);
    }
    Some(cost)
}

fn membership_label(membership: Option<&str>) -> Option<String> {
    membership.map(|t| match t.to_lowercase().as_str() {
        "enterprise" => "Cursor Enterprise".to_string(),
        "pro" => "Cursor Pro".to_string(),
        "hobby" => "Cursor Hobby".to_string(),
        "team" => "Cursor Team".to_string(),
        other => format!("Cursor {}", capitalize(other)),
    })
}

fn monthly_window_minutes(start: Option<&str>, end: Option<&str>) -> Option<u32> {
    let start = parse_iso_date(start?)?;
    let end = parse_iso_date(end?)?;
    let minutes = (end - start).num_minutes();
    (minutes > 0).then_some(minutes as u32)
}

fn parse_iso_date(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }

    if let Ok(dt) = chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ") {
        return Some(dt.with_timezone(&Utc));
    }

    None
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api() -> CursorApi {
        CursorApi::new()
    }

    fn parse_summary(json: &str) -> UsageSummary {
        serde_json::from_str(json).expect("fixture should parse")
    }

    fn extra<'a>(usage: &'a UsageSnapshot, id: &str) -> &'a NamedRateWindow {
        usage
            .extra_rate_windows
            .iter()
            .find(|w| w.id == id)
            .unwrap_or_else(|| panic!("expected extra window {id}"))
    }

    fn inactive<'a>(usage: &'a UsageSnapshot, id: &str) -> &'a InactiveRateWindow {
        usage
            .inactive_rate_windows
            .iter()
            .find(|w| w.id == id)
            .unwrap_or_else(|| panic!("expected inactive window {id}"))
    }

    #[test]
    fn test_cursor_build_result_with_lanes() {
        let json = r#"{
            "billingCycleStart": "2026-03-01T00:00:00Z",
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 1500,
                    "limit": 5000,
                    "totalPercentUsed": 30.0,
                    "autoPercentUsed": 20.0,
                    "apiPercentUsed": 10.0
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        assert!((usage.primary.used_percent - 30.0).abs() < 0.01);
        assert!(usage.primary.resets_at.is_some());
        assert_eq!(usage.primary.window_minutes, Some(31 * 24 * 60));

        let sec = usage.secondary.as_ref().expect("Auto should be present");
        assert!((sec.used_percent - 20.0).abs() < 0.01);
        assert!(sec.resets_at.is_some());

        let api_window = extra(&usage, "cursor-api");
        assert_eq!(api_window.title, "API");
        assert!((api_window.window.used_percent - 10.0).abs() < 0.01);
        assert!(api_window.window.resets_at.is_some());

        assert!(usage.inactive_rate_windows.is_empty());
        assert!(cost.is_some());
        assert_eq!(usage.login_method.as_deref(), Some("Cursor Pro"));
    }

    #[test]
    fn test_cursor_build_result_prefers_api_percent_fields() {
        let json = r#"{
            "membershipType": "pro",
            "autoModelSelectedDisplayMessage": "You've used 13% of your included total usage",
            "individualUsage": {
                "plan": {
                    "used": 2000,
                    "limit": 2000,
                    "breakdown": {
                        "included": 2000,
                        "bonus": 580,
                        "total": 2580
                    },
                    "autoPercentUsed": 17.2,
                    "apiPercentUsed": 0,
                    "totalPercentUsed": 13.230769230769232
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        assert!((usage.primary.used_percent - 13.230769230769232).abs() < 0.01);
        assert!((usage.secondary.as_ref().unwrap().used_percent - 17.2).abs() < 0.01);
        assert!((extra(&usage, "cursor-api").window.used_percent - 0.0).abs() < 0.01);

        let promo = extra(&usage, "cursor-promotional");
        assert_eq!(promo.title, "Promotional");
        // used == included, so promotional pool is untouched
        assert!((promo.window.used_percent - 0.0).abs() < 0.01);

        let cost = cost.expect("plan usage should still produce cost snapshot");
        assert!((cost.used - 20.0).abs() < 0.01);
        assert_eq!(cost.limit, Some(20.0));
        assert_eq!(usage.login_method.as_deref(), Some("Cursor Pro"));
    }

    #[test]
    fn test_cursor_promotional_partial_consumption() {
        let json = r#"{
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 2300,
                    "limit": 2580,
                    "breakdown": {
                        "included": 2000,
                        "bonus": 580,
                        "total": 2580
                    },
                    "totalPercentUsed": 89.14728682170542
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, _) = api().build_result(summary, None).unwrap();

        let promo = extra(&usage, "cursor-promotional");
        assert!((promo.window.used_percent - (300.0 / 580.0 * 100.0)).abs() < 0.01);
        assert!(promo.window.resets_at.is_some());

        // Lane format with only totalPercentUsed → Auto/API not enforced
        assert_eq!(inactive(&usage, "cursor-auto").description, NOT_ENFORCED);
        assert_eq!(inactive(&usage, "cursor-api").description, NOT_ENFORCED);
        assert!(usage.secondary.is_none());
        assert!(
            !usage
                .extra_rate_windows
                .iter()
                .any(|w| w.id == "cursor-api")
        );
    }

    #[test]
    fn test_cursor_build_result_cents_only() {
        let json = r#"{
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 2500,
                    "limit": 5000
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        assert!((usage.primary.used_percent - 50.0).abs() < 0.01);
        assert!(usage.secondary.is_none(), "no autoPercentUsed in payload");
        assert!(usage.extra_rate_windows.is_empty());
        assert!(
            usage.inactive_rate_windows.is_empty(),
            "cents-only payloads are not the percent-lane format"
        );
        assert!(cost.is_some());
    }

    #[test]
    fn test_cursor_build_result_missing_plan() {
        let json = r#"{
            "membershipType": "hobby",
            "individualUsage": {}
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        assert!((usage.primary.used_percent).abs() < 0.01);
        assert!(usage.secondary.is_none());
        assert!(usage.extra_rate_windows.is_empty());
        assert!(usage.inactive_rate_windows.is_empty());
        assert!(cost.is_none());
    }

    #[test]
    fn test_cursor_unlimited_without_monthly_meter() {
        let json = r#"{
            "membershipType": "hobby",
            "isUnlimited": true,
            "individualUsage": {}
        }"#;

        let summary = parse_summary(json);
        let (usage, _) = api().build_result(summary, None).unwrap();

        let monthly = inactive(&usage, "cursor-monthly");
        assert_eq!(monthly.title, "Monthly");
        assert_eq!(monthly.description, NOT_ENFORCED);
    }

    #[test]
    fn test_cursor_on_demand_as_named_extra_and_cost() {
        let json = r#"{
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 800,
                    "limit": 5000,
                    "totalPercentUsed": 16.0
                },
                "onDemand": {
                    "enabled": true,
                    "used": 350,
                    "limit": 1000
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        assert!((usage.primary.used_percent - 16.0).abs() < 0.01);

        let on_demand = extra(&usage, "cursor-on-demand");
        assert_eq!(on_demand.title, "On-demand");
        assert!((on_demand.window.used_percent - 35.0).abs() < 0.01);
        assert!(on_demand.window.resets_at.is_some());

        // Lane format with only total → Auto/API inactive
        assert_eq!(usage.inactive_rate_windows.len(), 2);

        let cost = cost.expect("cost should exist from on-demand usage");
        assert!((cost.used - 3.5).abs() < 0.01);
        assert_eq!(cost.limit, Some(10.0));
        assert_eq!(cost.period, "Monthly");
    }

    #[test]
    fn test_cursor_disabled_on_demand_is_ignored() {
        let json = r#"{
            "individualUsage": {
                "plan": {
                    "used": 100,
                    "limit": 1000,
                    "totalPercentUsed": 10.0,
                    "autoPercentUsed": 5.0,
                    "apiPercentUsed": 2.0
                },
                "onDemand": {
                    "enabled": false,
                    "used": 50,
                    "limit": 500
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        assert!(
            !usage
                .extra_rate_windows
                .iter()
                .any(|w| w.id == "cursor-on-demand")
        );
        // Falls back to plan cents when on-demand is disabled
        let cost = cost.expect("plan cost fallback");
        assert!((cost.used - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_cursor_individual_overall_fallback() {
        let summary =
            parse_summary(r#"{"individualUsage":{"overall":{"used":2500,"limit":10000}}}"#);
        let (usage, cost) = api().build_result(summary, None).unwrap();
        assert!((usage.primary.used_percent - 25.0).abs() < 0.01);
        assert_eq!(cost.unwrap().limit, Some(100.0));
        assert!(usage.extra_rate_windows.is_empty());
    }

    #[test]
    fn test_cursor_team_pooled_fallback() {
        let summary = parse_summary(r#"{"teamUsage":{"pooled":{"used":5000,"limit":10000}}}"#);
        let (usage, cost) = api().build_result(summary, None).unwrap();
        assert!((usage.primary.used_percent - 50.0).abs() < 0.01);
        assert_eq!(cost.unwrap().used, 50.0);
        assert!(usage.extra_rate_windows.is_empty());
    }

    #[test]
    fn test_cursor_does_not_invent_promotional_without_bonus() {
        let json = r#"{
            "individualUsage": {
                "plan": {
                    "used": 1000,
                    "limit": 2000,
                    "breakdown": {
                        "included": 2000,
                        "bonus": 0,
                        "total": 2000
                    },
                    "totalPercentUsed": 50.0,
                    "autoPercentUsed": 40.0,
                    "apiPercentUsed": 10.0
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, _) = api().build_result(summary, None).unwrap();
        assert!(
            !usage
                .extra_rate_windows
                .iter()
                .any(|w| w.id == "cursor-promotional")
        );
    }
}
