//! Cursor API client for fetching usage information
//!
//! Uses browser cookies to authenticate with cursor.com API

#[cfg(test)]
use crate::core::EnforcementState;
use crate::core::{
    CostSnapshot, InactiveRateWindow, NamedRateWindow, ProviderError, RateWindow, UsageSnapshot,
    WindowAmount,
};
use crate::providers::browser_cookie_header;
use chrono::{DateTime, Utc};
use serde::Deserialize;

const BASE_URL: &str = "https://cursor.com";
const COOKIE_DOMAINS: [&str; 2] = ["cursor.com", "cursor.sh"];
const NOT_ENFORCED: &str = "Not currently enforced by Cursor";
const ON_DEMAND_ID: &str = "cursor-on-demand";
const ON_DEMAND_TITLE: &str = "On-demand";
const ON_DEMAND_PERIOD: &str = "On-demand";
/// Plan / overall / pooled cents are Cursor internal units, not an invoice.
const INCLUDED_PERIOD: &str = "Included";
const NO_USAGE_REPORTED: &str = "No usage reported";

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

        // An empty `individualUsage: {}` object is not a reading. Fall through
        // to teamUsage instead of fabricating a 0% monthly meter.
        if let Some(individual) = summary
            .individual_usage
            .as_ref()
            .filter(|usage| individual_has_usage(usage))
        {
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
            } else if let Some(overall) = &individual.overall {
                // Overall is a single reported pool — keep it as monthly + cost only.
                monthly_percent = Self::usage_percent(overall);
                // Overall fills the cost slot as included units. On-demand still
                // surfaces as its own meter below; it must not replace this total.
                cost_snapshot = Self::on_demand_cost(Some(overall), billing_end, INCLUDED_PERIOD);
            }

            // On-demand is a sibling of plan/overall on IndividualUsage. Read it
            // once for every individual shape — including payloads that only
            // carry onDemand and omit both plan and overall.
            let on_demand = individual.on_demand.as_ref().or_else(|| {
                summary
                    .team_usage
                    .as_ref()
                    .and_then(|t| t.on_demand.as_ref())
            });
            let (metered, notice) = on_demand_windows(on_demand, window_minutes, billing_end);
            extras.extend(metered);
            inactives.extend(notice);

            // Cost ownership by shape:
            // - plan: on-demand (real money) outranks plan internal units
            // - overall: already set above; leave the monthly pool cost alone
            // - on-demand only: on-demand is the only billed signal
            if individual.plan.is_some() {
                cost_snapshot = Self::on_demand_cost(on_demand, billing_end, ON_DEMAND_PERIOD)
                    .or_else(|| {
                        individual
                            .plan
                            .as_ref()
                            .and_then(|plan| plan_cost_snapshot(plan, billing_end))
                    });
            } else if individual.overall.is_none() {
                cost_snapshot = Self::on_demand_cost(on_demand, billing_end, ON_DEMAND_PERIOD);
            }
        } else if let Some(team) = &summary.team_usage {
            if let Some(pooled) = &team.pooled {
                monthly_percent = Self::usage_percent(pooled);
            }
            let (metered, notice) =
                on_demand_windows(team.on_demand.as_ref(), window_minutes, billing_end);
            extras.extend(metered);
            inactives.extend(notice);
            // Same rule as the individual branch: on-demand is the billed lane,
            // so it takes the cost slot ahead of the pooled plan allowance.
            cost_snapshot =
                Self::on_demand_cost(team.on_demand.as_ref(), billing_end, ON_DEMAND_PERIOD);
            if cost_snapshot.is_none() {
                cost_snapshot = team.pooled.as_ref().and_then(|pooled| {
                    Self::on_demand_cost(Some(pooled), billing_end, INCLUDED_PERIOD)
                });
            }
        }

        let has_monthly_meter = monthly_percent.is_some();
        if !has_monthly_meter {
            // UsageSnapshot always needs a primary window, but 0% is not a
            // reading. Mark Monthly unavailable (or not enforced when Cursor
            // says the plan is unlimited) so glance surfaces can tell.
            if summary.is_unlimited == Some(true) {
                inactives.push(InactiveRateWindow::new(
                    "cursor-monthly",
                    "Monthly",
                    NOT_ENFORCED,
                ));
            } else {
                // Title must stay Plan so EnforcementTracker shares the
                // primary identity. "Monthly" would stick as a false
                // unavailable row after a later plan reading appears.
                inactives.push(InactiveRateWindow::unavailable(
                    "cursor-plan",
                    "Plan",
                    NO_USAGE_REPORTED,
                ));
            }
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
        period: &str,
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

        let mut cost = CostSnapshot::new(used_cents / 100.0, "USD", period);
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

fn individual_has_usage(individual: &IndividualUsage) -> bool {
    if individual.overall.is_some() || individual.on_demand.is_some() {
        return true;
    }
    individual.plan.as_ref().is_some_and(|plan| {
        plan_monthly_percent(plan).is_some()
            || plan.auto_percent_used.is_some()
            || plan.api_percent_used.is_some()
            || plan_cost_snapshot(plan, None).is_some()
    })
}

/// Cursor reports on-demand two ways: as a capped pool that can be metered, or
/// as uncapped spend with no limit at all. Uncapped overdraft is still real
/// money, so surface it as an explicit non-metering window instead of dropping
/// it on the floor for want of a denominator.
fn on_demand_windows(
    on_demand: Option<&OnDemandUsage>,
    window_minutes: Option<u32>,
    billing_end: Option<DateTime<Utc>>,
) -> (Option<NamedRateWindow>, Option<InactiveRateWindow>) {
    let Some(usage) = on_demand else {
        return (None, None);
    };
    if usage.enabled == Some(false) {
        return (None, None);
    }

    if let Some(percent) = CursorApi::usage_percent(usage) {
        let mut window = NamedRateWindow::new(
            ON_DEMAND_ID,
            ON_DEMAND_TITLE,
            RateWindow::with_details(percent, window_minutes, billing_end, None),
        );
        // The percentage drives the bar, but overdraft is money — show it.
        if let Some(amount) = on_demand_amount(usage) {
            window = window.with_amount(amount);
        }
        return (Some(window), None);
    }

    // No limit and no remaining: nothing to meter against, but the spend itself
    // is worth showing once the plan is exhausted.
    let used_cents = usage.used.unwrap_or(0);
    if used_cents <= 0 {
        return (None, None);
    }
    let spent = WindowAmount::new(used_cents as f64 / 100.0, "USD").format_used();
    let notice = InactiveRateWindow::new(
        ON_DEMAND_ID,
        ON_DEMAND_TITLE,
        format!("{spent} spent, no spend limit set"),
    );
    (None, Some(notice))
}

/// The plan lane expressed in currency.
///
/// Note this is *not* money owed. Cursor prices included usage at internal
/// rates, and an exported usage report shows every such event as "Included" or
/// "Free" with no charge. Only the on-demand lane is real billing, which is why
/// it takes the cost slot ahead of this.
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

    let mut cost = CostSnapshot::new(used_cents / 100.0, "USD", INCLUDED_PERIOD);
    if limit_cents > 0.0 {
        cost = cost.with_limit(limit_cents / 100.0);
    }
    if let Some(reset) = billing_end {
        cost = cost.with_resets_at(reset);
    }
    Some(cost)
}

/// Dollars behind an on-demand lane, for display beside its meter.
fn on_demand_amount(usage: &OnDemandUsage) -> Option<WindowAmount> {
    // Treat an omitted `used` as zero, the way `usage_percent` does, so a
    // reported cap still renders instead of the amount vanishing entirely.
    let used = usage.used.unwrap_or(0) as f64;
    let mut amount = WindowAmount::new(used / 100.0, "USD");
    if let Some(limit) = usage
        .limit
        .or_else(|| usage.remaining.map(|r| r + usage.used.unwrap_or(0)))
        .filter(|&l| l > 0)
    {
        amount = amount.with_limit(limit as f64 / 100.0);
    }
    Some(amount)
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

        // No Promotional lane: `breakdown.bonus` is bonus *consumed*, and the
        // grant it would be metered against is nowhere in the payload.
        assert!(
            !usage
                .extra_rate_windows
                .iter()
                .any(|w| w.id == "cursor-promotional")
        );

        let cost = cost.expect("plan usage should still produce cost snapshot");
        assert!((cost.used - 20.0).abs() < 0.01);
        assert_eq!(cost.limit, Some(20.0));
        assert_eq!(usage.login_method.as_deref(), Some("Cursor Pro"));
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
    fn test_cursor_percent_falls_back_to_the_breakdown_total_limit() {
        // No `totalPercentUsed` and no `plan.limit`: the percentage falls back
        // to metering `used` against `breakdown.total`.
        let json = r#"{
            "individualUsage": {
                "plan": {
                    "used": 1250,
                    "breakdown": {"included": 5000, "bonus": 0, "total": 5000}
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, _) = api().build_result(summary, None).unwrap();
        assert!((usage.primary.used_percent - 25.0).abs() < 0.01);
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
        let monthly = inactive(&usage, "cursor-plan");
        assert_eq!(monthly.title, "Plan");
        assert_eq!(monthly.description, NO_USAGE_REPORTED);
        assert_eq!(monthly.state, EnforcementState::Unavailable);
        assert!(cost.is_none());
    }

    #[test]
    fn test_cursor_on_demand_only_individual_usage() {
        // individualUsage may carry only onDemand (plan and overall both null).
        // That shape used to fall through both arms and drop the overdraft lane.
        let json = r#"{
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "onDemand": {
                    "enabled": true,
                    "used": 350,
                    "limit": 1000
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        let on_demand = extra(&usage, "cursor-on-demand");
        assert_eq!(on_demand.title, "On-demand");
        assert!((on_demand.window.used_percent - 35.0).abs() < 0.01);

        let cost = cost.expect("on-demand-only payload still reports spend");
        assert!((cost.used - 3.5).abs() < 0.01);
        assert_eq!(cost.limit, Some(10.0));
        assert_eq!(cost.period, "On-demand");
        // No plan/overall reading: Monthly is missing, not 0%.
        let monthly = inactive(&usage, "cursor-plan");
        assert_eq!(monthly.title, "Plan");
        assert_eq!(monthly.state, EnforcementState::Unavailable);
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

        // The overdraft meter carries its own dollars, so it stays visible even
        // though the single cost slot belongs to plan spend.
        let amount = on_demand.amount.as_ref().expect("on-demand carries money");
        assert!((amount.used - 3.50).abs() < 0.01);
        assert_eq!(amount.limit, Some(10.0));
        assert_eq!(amount.format_used(), "$3.50");

        // Lane format with only total â†’ Auto/API inactive
        assert_eq!(usage.inactive_rate_windows.len(), 2);

        // On-demand is the only real-money lane, so it owns the cost slot.
        let cost = cost.expect("on-demand cost");
        assert!((cost.used - 3.5).abs() < 0.01);
        assert_eq!(cost.limit, Some(10.0));
        assert_eq!(cost.period, "On-demand");
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
    fn test_cursor_reports_no_promotional_meter() {
        // The old meter computed `plan.used - breakdown.included`, which is
        // structurally zero: the included lane is its own closed set, so it
        // read 0% forever. `breakdown.bonus` is bonus consumed, not a grant,
        // so there is nothing honest to meter it against.
        let json = r#"{
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 2000,
                    "limit": 2000,
                    "remaining": 0,
                    "breakdown": {
                        "included": 2000,
                        "bonus": 23466,
                        "total": 25466
                    },
                    "totalPercentUsed": 73.81449275362318
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
        // And no promo badge, which carried a billing-cycle end date while
        // Cursor credits expire on their own schedule.
        assert!(usage.promo_signals.is_empty());
        // The primary meter is unaffected: it reads totalPercentUsed directly.
        assert!((usage.primary.used_percent - 73.81449275362318).abs() < 0.01);
    }

    #[test]
    fn test_cursor_uncapped_on_demand_reports_spend() {
        // On-demand enabled with real spend but no cap: there is no denominator
        // to meter, so the spend surfaces as an explicit non-metering window.
        let json = r#"{
            "billingCycleEnd": "2026-09-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 2000,
                    "limit": 2000,
                    "totalPercentUsed": 100.0
                },
                "onDemand": {
                    "enabled": true,
                    "used": 1234
                }
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        assert!(
            !usage
                .extra_rate_windows
                .iter()
                .any(|w| w.id == "cursor-on-demand"),
            "no cap means no meter"
        );
        let notice = inactive(&usage, "cursor-on-demand");
        assert_eq!(notice.title, "On-demand");
        assert_eq!(notice.description, "$12.34 spent, no spend limit set");

        let cost = cost.expect("uncapped on-demand still reports spend");
        assert!((cost.used - 12.34).abs() < 0.01);
        assert_eq!(cost.limit, None);
        assert_eq!(cost.period, "On-demand");
    }

    #[test]
    fn test_cursor_on_demand_surfaces_alongside_overall() {
        let json = r#"{
            "individualUsage": {
                "overall": {"used": 2500, "limit": 2500},
                "onDemand": {"enabled": true, "used": 900, "limit": 5000}
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        assert!((usage.primary.used_percent - 100.0).abs() < 0.01);
        let on_demand = extra(&usage, "cursor-on-demand");
        assert!((on_demand.window.used_percent - 18.0).abs() < 0.01);

        // `overall` remains the reported total pool, labeled as included
        // units rather than billed on-demand spend.
        let cost = cost.expect("overall cost");
        assert!((cost.used - 25.0).abs() < 0.01);
        assert_eq!(cost.period, INCLUDED_PERIOD);
    }

    #[test]
    fn test_cursor_zero_spend_uncapped_on_demand_is_silent() {
        let json = r#"{
            "individualUsage": {
                "plan": {"used": 100, "limit": 1000, "totalPercentUsed": 10.0},
                "onDemand": {"enabled": true, "used": 0}
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, _) = api().build_result(summary, None).unwrap();

        assert!(
            !usage
                .inactive_rate_windows
                .iter()
                .any(|w| w.id == "cursor-on-demand")
        );
        assert!(
            !usage
                .extra_rate_windows
                .iter()
                .any(|w| w.id == "cursor-on-demand")
        );
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
    fn test_cursor_on_demand_cap_renders_without_a_used_field() {
        // Cursor can report a cap with no `used`. That is zero spend against a
        // real limit, not an absent amount.
        let json = r#"{
            "individualUsage": {
                "plan": {"used": 100, "limit": 1000, "totalPercentUsed": 10.0},
                "onDemand": {"enabled": true, "limit": 2000}
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, _) = api().build_result(summary, None).unwrap();

        let amount = extra(&usage, "cursor-on-demand")
            .amount
            .as_ref()
            .expect("a cap is still an amount");
        assert_eq!(amount.used, 0.0);
        assert_eq!(amount.limit, Some(20.0));
    }

    #[test]
    fn test_cursor_team_on_demand_outranks_pooled_for_cost() {
        // On-demand is the billed lane for teams too, so it must not be
        // shadowed by the pooled plan allowance.
        let json = r#"{
            "teamUsage": {
                "pooled": {"used": 5000, "limit": 10000},
                "onDemand": {"enabled": true, "used": 750, "limit": 2000}
            }
        }"#;

        let summary = parse_summary(json);
        let (usage, cost) = api().build_result(summary, None).unwrap();

        assert!((usage.primary.used_percent - 50.0).abs() < 0.01);
        let cost = cost.expect("on-demand cost");
        assert!((cost.used - 7.5).abs() < 0.01);
        assert_eq!(cost.period, "On-demand");
    }

    #[test]
    fn test_cursor_team_pooled_fallback() {
        let summary = parse_summary(r#"{"teamUsage":{"pooled":{"used":5000,"limit":10000}}}"#);
        let (usage, cost) = api().build_result(summary, None).unwrap();
        assert!((usage.primary.used_percent - 50.0).abs() < 0.01);
        let cost = cost.expect("pooled included units");
        assert_eq!(cost.used, 50.0);
        assert_eq!(cost.period, INCLUDED_PERIOD);
        assert!(usage.extra_rate_windows.is_empty());
    }

    #[test]
    fn cursor_usage_fixtures_cover_normal_partial_duplicate_and_malformed() {
        let (normal, normal_cost) = api()
            .build_result(
                parse_summary(include_str!("../fixtures/cursor/normal.json")),
                None,
            )
            .expect("normal fixture");
        assert!((normal.primary.used_percent - 16.0).abs() < 0.01);
        assert!((normal.secondary.as_ref().unwrap().used_percent - 20.0).abs() < 0.01);
        assert!((extra(&normal, "cursor-api").window.used_percent - 10.0).abs() < 0.01);
        let billed = extra(&normal, ON_DEMAND_ID);
        assert_eq!(billed.title, ON_DEMAND_TITLE);
        assert!((billed.window.used_percent - 35.0).abs() < 0.01);
        let cost = normal_cost.expect("on-demand owns the cost slot");
        assert_eq!(cost.period, ON_DEMAND_PERIOD);
        assert!((cost.used - 3.5).abs() < 0.01);
        assert!(
            !normal
                .inactive_rate_windows
                .iter()
                .any(|window| window.id == "cursor-monthly")
        );

        let (partial, partial_cost) = api()
            .build_result(
                parse_summary(include_str!("../fixtures/cursor/partial.json")),
                None,
            )
            .expect("partial fixture");
        assert!(partial.secondary.is_none());
        assert!((extra(&partial, ON_DEMAND_ID).window.used_percent - 35.0).abs() < 0.01);
        assert_eq!(
            partial_cost.expect("partial still bills on-demand").period,
            ON_DEMAND_PERIOD
        );
        let monthly = inactive(&partial, "cursor-plan");
        assert_eq!(monthly.title, "Plan");
        assert_eq!(monthly.state, EnforcementState::Unavailable);
        assert_eq!(monthly.description, NO_USAGE_REPORTED);

        let (duplicate, duplicate_cost) = api()
            .build_result(
                parse_summary(include_str!("../fixtures/cursor/duplicate.json")),
                None,
            )
            .expect("duplicate fixture");
        // Individual wins when both individual and team payloads are present.
        assert!((duplicate.primary.used_percent - 16.0).abs() < 0.01);
        assert!((extra(&duplicate, ON_DEMAND_ID).window.used_percent - 35.0).abs() < 0.01);
        let duplicate_cost = duplicate_cost.expect("individual on-demand");
        assert_eq!(duplicate_cost.period, ON_DEMAND_PERIOD);
        assert!(
            (duplicate_cost.used - 3.5).abs() < 0.01,
            "team on-demand $7.50 must not replace or add to individual $3.50"
        );

        let (malformed, malformed_cost) = api()
            .build_result(
                parse_summary(include_str!("../fixtures/cursor/malformed.json")),
                None,
            )
            .expect("malformed-but-parseable fixture");
        assert!(malformed.primary.resets_at.is_none());
        assert!(malformed.primary.window_minutes.is_none());
        assert!((malformed.primary.used_percent - 12.5).abs() < 0.01);
        let amount = extra(&malformed, ON_DEMAND_ID)
            .amount
            .as_ref()
            .expect("a reported cap is still an amount");
        assert_eq!(amount.used, 0.0);
        assert_eq!(amount.limit, Some(20.0));
        assert_eq!(
            malformed_cost.expect("cap-only on-demand").period,
            ON_DEMAND_PERIOD
        );
    }

    #[test]
    fn empty_individual_usage_falls_through_to_team_instead_of_zero() {
        let (usage, cost) = api()
            .build_result(
                parse_summary(include_str!(
                    "../fixtures/cursor/empty-individual-with-team.json"
                )),
                None,
            )
            .expect("empty individual + team");
        assert!((usage.primary.used_percent - 50.0).abs() < 0.01);
        let cost = cost.expect("team pooled included units");
        assert_eq!(cost.period, INCLUDED_PERIOD);
        assert!((cost.used - 50.0).abs() < 0.01);
        assert!(
            !usage
                .inactive_rate_windows
                .iter()
                .any(|window| window.id == "cursor-monthly" || window.id == "cursor-plan")
        );
    }

    #[test]
    fn empty_plan_object_falls_through_to_team() {
        let (usage, cost) = api()
            .build_result(
                parse_summary(include_str!("../fixtures/cursor/empty-plan-with-team.json")),
                None,
            )
            .expect("empty plan + team");
        assert!((usage.primary.used_percent - 50.0).abs() < 0.01);
        let cost = cost.expect("team pooled included units");
        assert_eq!(cost.period, INCLUDED_PERIOD);
        assert!(
            !usage
                .inactive_rate_windows
                .iter()
                .any(|window| window.id == "cursor-plan")
        );
    }

    #[test]
    fn unparseable_cursor_summary_fails_closed() {
        let err = serde_json::from_str::<UsageSummary>(include_str!(
            "../fixtures/cursor/unparseable.json"
        ))
        .expect_err("wrong-typed percent must not become 0%");
        assert!(err.to_string().contains("invalid type"));
    }
}
