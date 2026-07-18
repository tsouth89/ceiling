use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::commands::{ProviderUsageSnapshot, RateWindowSnapshot};

const USED_DROP_THRESHOLD: f64 = 20.0;
const RESET_JITTER_MINUTES: i64 = 10;
const RESET_SHIFT_MINUTES: i64 = 30;
const CONFIRM_USED_TOLERANCE: f64 = 10.0;
const CONFIRMATION_MIN_AGE_SECONDS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CapacityEventKind {
    ScheduledReset,
    SurpriseReset,
    PartialReset,
    ResetTimeShift,
    WindowLifted,
    WindowRestored,
    AllowanceGranted,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityEventPayload {
    pub provider_id: String,
    pub display_name: String,
    pub window_id: String,
    pub window_label: String,
    pub kind: CapacityEventKind,
    pub previous_used_percent: f64,
    pub current_used_percent: f64,
    pub previous_reset_at: String,
    pub current_reset_at: String,
    pub occurred_at: String,
}

impl CapacityEventPayload {
    pub fn notification_title(&self) -> String {
        match self.kind {
            CapacityEventKind::ScheduledReset => format!("{} reset", self.display_name),
            CapacityEventKind::SurpriseReset => {
                format!("{} capacity restored early", self.display_name)
            }
            CapacityEventKind::PartialReset => {
                format!("{} capacity partially restored", self.display_name)
            }
            CapacityEventKind::ResetTimeShift => {
                format!("{} reset time changed", self.display_name)
            }
            CapacityEventKind::WindowLifted => {
                format!("{} limit lifted", self.display_name)
            }
            CapacityEventKind::WindowRestored => {
                format!("{} limit restored", self.display_name)
            }
            CapacityEventKind::AllowanceGranted => {
                format!("{} capacity added", self.display_name)
            }
        }
    }

    pub fn notification_body(&self) -> String {
        let remaining = (100.0 - self.current_used_percent).clamp(0.0, 100.0);
        match self.kind {
            CapacityEventKind::ScheduledReset => format!(
                "{} reset on schedule. {:.0}% available now.",
                self.window_label, remaining
            ),
            CapacityEventKind::SurpriseReset => format!(
                "{} reset earlier than expected. {:.0}% available now.",
                self.window_label, remaining
            ),
            CapacityEventKind::PartialReset => format!(
                "{} dropped from {:.0}% to {:.0}% used. {:.0}% available now.",
                self.window_label, self.previous_used_percent, self.current_used_percent, remaining
            ),
            CapacityEventKind::ResetTimeShift => {
                format!("{} now has a different reset time.", self.window_label)
            }
            CapacityEventKind::WindowLifted => {
                format!(
                    "{} is no longer reporting an active limit.",
                    self.window_label
                )
            }
            CapacityEventKind::WindowRestored => {
                format!("{} is reporting an active limit again.", self.window_label)
            }
            CapacityEventKind::AllowanceGranted => format!(
                "{} is newly available with {:.0}% remaining.",
                self.window_label, remaining
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObservedWindow {
    id: String,
    label: String,
    used_percent: f64,
    resets_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderObservation {
    observed_at: DateTime<Utc>,
    windows: HashMap<String, ObservedWindow>,
    #[serde(default)]
    inactive_windows: HashMap<String, String>,
    #[serde(default)]
    extra_window_ids: HashSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingReset {
    event: PersistedEvent,
    candidate: ObservedWindow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CandidateState {
    Active(ObservedWindow),
    Inactive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingTransition {
    event: PersistedEvent,
    candidate: CandidateState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedEvent {
    provider_id: String,
    display_name: String,
    window_id: String,
    window_label: String,
    kind: CapacityEventKind,
    previous_used_percent: f64,
    current_used_percent: f64,
    previous_reset_at: DateTime<Utc>,
    current_reset_at: DateTime<Utc>,
    occurred_at: DateTime<Utc>,
}

impl PersistedEvent {
    fn payload(self) -> CapacityEventPayload {
        CapacityEventPayload {
            provider_id: self.provider_id,
            display_name: self.display_name,
            window_id: self.window_id,
            window_label: self.window_label,
            kind: self.kind,
            previous_used_percent: self.previous_used_percent,
            current_used_percent: self.current_used_percent,
            previous_reset_at: self.previous_reset_at.to_rfc3339(),
            current_reset_at: self.current_reset_at.to_rfc3339(),
            occurred_at: self.occurred_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CapacityEventObserver {
    baselines: HashMap<String, ProviderObservation>,
    /// Confirmation candidates are process-local. Persisting them replays an
    /// unfinished event from the previous run as a fresh launch notification.
    #[serde(skip)]
    pending_resets: HashMap<String, PendingReset>,
    #[serde(skip)]
    pending_transitions: HashMap<String, PendingTransition>,
    /// Every provider/account scope is re-baselined on its first live reading
    /// after launch so changes that happened while Ceiling was closed are not
    /// emitted as if they just occurred.
    #[serde(skip)]
    seen_scopes: HashSet<String>,
    #[serde(skip)]
    persistence_path: Option<PathBuf>,
}

impl CapacityEventObserver {
    pub fn load_default() -> Self {
        let path = persistence_path();
        let Some(path_ref) = path.as_ref() else {
            return Self::default();
        };
        let mut observer = fs::read_to_string(path_ref)
            .ok()
            .and_then(|contents| serde_json::from_str::<Self>(&contents).ok())
            .unwrap_or_default();
        // Explicitly discard candidates written by older builds.
        observer.pending_resets.clear();
        observer.pending_transitions.clear();
        observer.persistence_path = path;
        observer
    }

    pub fn observe(&mut self, snapshot: &ProviderUsageSnapshot) -> Vec<CapacityEventPayload> {
        if snapshot.error.is_some() {
            return Vec::new();
        }
        let now = DateTime::parse_from_rfc3339(&snapshot.updated_at)
            .map(|value| value.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let scope = observation_scope(snapshot);
        let (windows, extra_window_ids) = observed_windows(snapshot);
        let current = ProviderObservation {
            observed_at: now,
            windows,
            inactive_windows: inactive_windows(snapshot),
            extra_window_ids,
        };

        if self.seen_scopes.insert(scope.clone()) {
            self.baselines.insert(scope, current);
            self.persist();
            return Vec::new();
        }

        let Some(previous) = self.baselines.get(&scope).cloned() else {
            self.baselines.insert(scope, current);
            self.persist();
            return Vec::new();
        };

        let mut emitted = Vec::new();
        let mut held_for_confirmation = false;
        let mut confirmed_windows = HashSet::new();
        for (window_id, current_window) in &current.windows {
            let pending_key = format!("{scope}:{window_id}");
            if let Some(pending) = self.pending_resets.get(&pending_key).cloned() {
                if consistent_confirmation(&pending.candidate, current_window) {
                    if confirmation_is_mature(pending.event.occurred_at, current.observed_at) {
                        self.pending_resets.remove(&pending_key);
                        emitted.push(pending.event.payload());
                        confirmed_windows.insert(window_id.clone());
                    } else {
                        held_for_confirmation = true;
                    }
                    continue;
                }
                self.pending_resets.remove(&pending_key);
            }

            let Some(previous_window) = previous.windows.get(window_id) else {
                continue;
            };
            let Some(event) = detect_reset(
                snapshot,
                &previous,
                &current,
                previous_window,
                current_window,
            ) else {
                continue;
            };
            if event.kind == CapacityEventKind::ScheduledReset {
                emitted.push(event.payload());
            } else {
                self.pending_resets.insert(
                    pending_key,
                    PendingReset {
                        event,
                        candidate: current_window.clone(),
                    },
                );
                held_for_confirmation = true;
            }
        }

        for window_id in transition_window_ids(&previous, &current) {
            if confirmed_windows.contains(&window_id) {
                continue;
            }
            let pending_key = format!("{scope}:transition:{window_id}");
            if let Some(pending) = self.pending_transitions.get(&pending_key).cloned() {
                if transition_is_consistent(&pending.candidate, &current, &window_id) {
                    if confirmation_is_mature(pending.event.occurred_at, current.observed_at) {
                        self.pending_transitions.remove(&pending_key);
                        emitted.push(pending.event.payload());
                        confirmed_windows.insert(window_id.clone());
                    } else {
                        held_for_confirmation = true;
                    }
                    continue;
                }
                self.pending_transitions.remove(&pending_key);
            }

            let Some((event, candidate)) =
                detect_transition(snapshot, &previous, &current, &window_id)
            else {
                continue;
            };
            self.pending_transitions
                .insert(pending_key, PendingTransition { event, candidate });
            held_for_confirmation = true;
        }

        if !held_for_confirmation {
            self.baselines.insert(scope, current);
        }
        self.persist();
        emitted
    }

    fn persist(&self) {
        let Some(path) = self.persistence_path.as_ref() else {
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            tracing::warn!("failed to create capacity-event directory: {error}");
            return;
        }
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                if let Err(error) = fs::write(path, bytes) {
                    tracing::warn!("failed to persist capacity-event history: {error}");
                }
            }
            Err(error) => tracing::warn!("failed to serialize capacity-event history: {error}"),
        }
    }
}

fn detect_reset(
    snapshot: &ProviderUsageSnapshot,
    previous_observation: &ProviderObservation,
    current_observation: &ProviderObservation,
    previous: &ObservedWindow,
    current: &ObservedWindow,
) -> Option<PersistedEvent> {
    let used_drop = previous.used_percent - current.used_percent;
    let reset_advanced =
        current.resets_at > previous.resets_at + Duration::minutes(RESET_JITTER_MINUTES);
    let scheduled = previous.resets_at >= previous_observation.observed_at - Duration::minutes(5)
        && previous.resets_at <= current_observation.observed_at + Duration::minutes(5)
        && reset_advanced;
    let reset_shift =
        (current.resets_at - previous.resets_at).num_minutes().abs() >= RESET_SHIFT_MINUTES;
    let reset_unchanged =
        (current.resets_at - previous.resets_at).num_minutes().abs() <= RESET_JITTER_MINUTES;
    let kind = if scheduled {
        CapacityEventKind::ScheduledReset
    } else if used_drop >= USED_DROP_THRESHOLD && reset_advanced {
        CapacityEventKind::SurpriseReset
    } else if used_drop >= USED_DROP_THRESHOLD && reset_unchanged {
        // Some providers restore only part of a pool without moving its normal
        // reset date. Treat a large, confirmed decrease in used capacity as a
        // real event while leaving small corrections and reset-time churn alone.
        CapacityEventKind::PartialReset
    } else if reset_shift {
        CapacityEventKind::ResetTimeShift
    } else {
        return None;
    };
    Some(PersistedEvent {
        provider_id: snapshot.provider_id.clone(),
        display_name: snapshot.display_name.clone(),
        window_id: current.id.clone(),
        window_label: current.label.clone(),
        kind,
        previous_used_percent: previous.used_percent,
        current_used_percent: current.used_percent,
        previous_reset_at: previous.resets_at,
        current_reset_at: current.resets_at,
        occurred_at: current_observation.observed_at,
    })
}

fn consistent_confirmation(candidate: &ObservedWindow, current: &ObservedWindow) -> bool {
    (candidate.used_percent - current.used_percent).abs() <= CONFIRM_USED_TOLERANCE
        && (candidate.resets_at - current.resets_at)
            .num_minutes()
            .abs()
            <= RESET_JITTER_MINUTES
}

fn confirmation_is_mature(candidate_at: DateTime<Utc>, current_at: DateTime<Utc>) -> bool {
    current_at - candidate_at >= Duration::seconds(CONFIRMATION_MIN_AGE_SECONDS)
}

fn transition_window_ids(
    previous: &ProviderObservation,
    current: &ProviderObservation,
) -> HashSet<String> {
    previous
        .windows
        .keys()
        .chain(previous.inactive_windows.keys())
        .chain(current.windows.keys())
        .chain(current.inactive_windows.keys())
        .cloned()
        .collect()
}

fn transition_is_consistent(
    candidate: &CandidateState,
    current: &ProviderObservation,
    window_id: &str,
) -> bool {
    match candidate {
        CandidateState::Active(window) => current
            .windows
            .get(window_id)
            .is_some_and(|current| consistent_confirmation(window, current)),
        CandidateState::Inactive => current.inactive_windows.contains_key(window_id),
    }
}

fn detect_transition(
    snapshot: &ProviderUsageSnapshot,
    previous: &ProviderObservation,
    current: &ProviderObservation,
    window_id: &str,
) -> Option<(PersistedEvent, CandidateState)> {
    let previous_active = previous.windows.get(window_id);
    let current_active = current.windows.get(window_id);
    let previous_inactive = previous.inactive_windows.get(window_id);
    let current_inactive = current.inactive_windows.get(window_id);

    let (kind, label, candidate) =
        if let (Some(_), Some(label)) = (previous_active, current_inactive) {
            (
                CapacityEventKind::WindowLifted,
                label.clone(),
                CandidateState::Inactive,
            )
        } else if let (Some(label), Some(current)) = (previous_inactive, current_active) {
            (
                CapacityEventKind::WindowRestored,
                label.clone(),
                CandidateState::Active(current.clone()),
            )
        } else if previous_active.is_none()
            && previous_inactive.is_none()
            && current.extra_window_ids.contains(window_id)
        {
            let current = current_active?;
            (
                CapacityEventKind::AllowanceGranted,
                current.label.clone(),
                CandidateState::Active(current.clone()),
            )
        } else {
            return None;
        };

    let previous_used_percent = previous_active.map_or(0.0, |window| window.used_percent);
    let current_used_percent = current_active.map_or(0.0, |window| window.used_percent);
    let previous_reset_at = previous_active
        .map(|window| window.resets_at)
        .unwrap_or(previous.observed_at);
    let current_reset_at = current_active
        .map(|window| window.resets_at)
        .unwrap_or(current.observed_at);
    Some((
        PersistedEvent {
            provider_id: snapshot.provider_id.clone(),
            display_name: snapshot.display_name.clone(),
            window_id: window_id.to_string(),
            window_label: label,
            kind,
            previous_used_percent,
            current_used_percent,
            previous_reset_at,
            current_reset_at,
            occurred_at: current.observed_at,
        },
        candidate,
    ))
}

fn observed_windows(
    snapshot: &ProviderUsageSnapshot,
) -> (HashMap<String, ObservedWindow>, HashSet<String>) {
    let mut windows = HashMap::new();
    let mut extra_window_ids = HashSet::new();
    push_window(
        &mut windows,
        snapshot.primary_label.as_deref().unwrap_or("Plan"),
        &snapshot.primary,
    );
    if let Some(window) = snapshot.secondary.as_ref() {
        push_window(
            &mut windows,
            snapshot.secondary_label.as_deref().unwrap_or("Secondary"),
            window,
        );
    }
    for extra in &snapshot.extra_rate_windows {
        if ignored_capacity_window(snapshot, &extra.id, &extra.title) {
            continue;
        }
        let id = semantic_inactive_window_id(&snapshot.provider_id, &extra.id, &extra.title);
        if let Some(mut observed) = to_observed_window(&extra.title, &extra.window) {
            observed.id.clone_from(&id);
            windows.insert(id.clone(), observed);
            extra_window_ids.insert(id);
        }
    }
    (windows, extra_window_ids)
}

fn inactive_windows(snapshot: &ProviderUsageSnapshot) -> HashMap<String, String> {
    snapshot
        .inactive_rate_windows
        .iter()
        .filter(|window| !ignored_capacity_window(snapshot, &window.id, &window.title))
        .map(|window| {
            (
                semantic_inactive_window_id(&snapshot.provider_id, &window.id, &window.title),
                window.title.clone(),
            )
        })
        .collect()
}

pub(crate) fn ignored_capacity_window(
    snapshot: &ProviderUsageSnapshot,
    id: &str,
    title: &str,
) -> bool {
    if snapshot.provider_id != "cursor" {
        return false;
    }
    let identity = normalize_window_id(&format!("{id}-{title}"));
    identity.contains("promotional")
        || identity.contains("on-demand")
        || identity.contains("ondemand")
}

pub(crate) fn semantic_inactive_window_id(provider_id: &str, id: &str, title: &str) -> String {
    let title_id = normalize_window_id(title);
    if let Some(core_id) = core_window_id(&title_id) {
        return core_id.to_string();
    }
    // Named extra allowances must keep their own identity even when their
    // cadence is weekly/monthly. Otherwise Codex Spark Weekly overwrites the
    // regular Codex Weekly baseline and hides a real reset.
    if !title_id.is_empty() {
        return title_id;
    }
    let normalized = normalize_window_id(id);
    let without_provider = normalized
        .strip_prefix(&format!("{}-", normalize_window_id(provider_id)))
        .unwrap_or(&normalized);
    core_window_id(without_provider)
        .unwrap_or(without_provider)
        .to_string()
}

fn core_window_id(normalized: &str) -> Option<&'static str> {
    match normalized {
        "auto" => Some("auto"),
        "api" => Some("api"),
        "total" => Some("total"),
        "plan" => Some("plan"),
        "weekly" => Some("weekly"),
        "monthly" => Some("monthly"),
        "session" | "session-5h" | "session-5-hour" | "5-hour" | "five-hour" => Some("session"),
        _ => None,
    }
}

fn push_window(
    windows: &mut HashMap<String, ObservedWindow>,
    label: &str,
    window: &RateWindowSnapshot,
) {
    if let Some(observed) = to_observed_window(label, window) {
        windows.insert(observed.id.clone(), observed);
    }
}

fn to_observed_window(label: &str, window: &RateWindowSnapshot) -> Option<ObservedWindow> {
    let resets_at = window
        .resets_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())?
        .with_timezone(&Utc);
    Some(ObservedWindow {
        id: semantic_window_id(label, window.window_minutes),
        label: label.to_string(),
        used_percent: window.used_percent,
        resets_at,
    })
}

pub(crate) fn semantic_window_id(label: &str, window_minutes: Option<u32>) -> String {
    let normalized = normalize_window_id(label);
    if matches!(
        normalized.as_str(),
        "auto" | "api" | "total" | "plan" | "weekly" | "monthly"
    ) {
        return normalized;
    }
    match window_minutes {
        Some(minutes) if minutes <= 720 => "session".to_string(),
        Some(minutes) if minutes <= 20_160 => "weekly".to_string(),
        Some(_) => "monthly".to_string(),
        None => normalized,
    }
}

fn normalize_window_id(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

pub(crate) fn observation_scope(snapshot: &ProviderUsageSnapshot) -> String {
    let identity = snapshot
        .account_email
        .as_deref()
        .or(snapshot.account_organization.as_deref())
        .unwrap_or("anonymous");
    let raw = format!(
        "{}|{}|{}",
        snapshot.provider_id, snapshot.source_label, identity
    );
    format!("{}:{:016x}", snapshot.provider_id, fnv1a64(raw.as_bytes()))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn persistence_path() -> Option<PathBuf> {
    codexbar::settings::Settings::settings_path().and_then(|path| {
        path.parent()
            .map(|parent| parent.join("capacity-events.json"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{
        InactiveRateWindowSnapshot, NamedRateWindowSnapshot, ProviderUsageSnapshot,
    };

    fn window(used: f64, resets_at: DateTime<Utc>) -> RateWindowSnapshot {
        RateWindowSnapshot {
            used_percent: used,
            remaining_percent: 100.0 - used,
            window_minutes: Some(300),
            resets_at: Some(resets_at.to_rfc3339()),
            reset_description: None,
            is_exhausted: false,
            reserve_percent: None,
            reserve_description: None,
            reserve_will_last_to_reset: false,
            reserve_eta_seconds: None,
        }
    }

    fn snapshot(at: DateTime<Utc>, used: f64, reset: DateTime<Utc>) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider_id: "codex".into(),
            display_name: "Codex".into(),
            primary: window(used, reset),
            primary_label: Some("Session".into()),
            secondary: None,
            secondary_label: None,
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
            updated_at: at.to_rfc3339(),
            error: None,
            pace: None,
            account_organization: None,
            tray_status_label: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
        }
    }

    fn with_extra(
        mut snapshot: ProviderUsageSnapshot,
        id: &str,
        title: &str,
        used: f64,
        reset: DateTime<Utc>,
    ) -> ProviderUsageSnapshot {
        snapshot.extra_rate_windows.push(NamedRateWindowSnapshot {
            id: id.into(),
            title: title.into(),
            window: window(used, reset),
        });
        snapshot
    }

    fn with_inactive(
        mut snapshot: ProviderUsageSnapshot,
        id: &str,
        title: &str,
    ) -> ProviderUsageSnapshot {
        snapshot
            .inactive_rate_windows
            .push(InactiveRateWindowSnapshot {
                id: id.into(),
                title: title.into(),
                description: "Not currently limited".into(),
                state: "notEnforced".into(),
            });
        snapshot
    }

    #[test]
    fn surprise_reset_requires_a_consistent_second_read() {
        let start = Utc::now();
        let old_reset = start + Duration::hours(4);
        let new_reset = start + Duration::hours(9);
        let mut observer = CapacityEventObserver::default();

        assert!(
            observer
                .observe(&snapshot(start, 85.0, old_reset))
                .is_empty()
        );
        assert!(
            observer
                .observe(&snapshot(start + Duration::minutes(5), 10.0, new_reset))
                .is_empty()
        );
        let events = observer.observe(&snapshot(
            start + Duration::minutes(10),
            12.0,
            new_reset + Duration::minutes(2),
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, CapacityEventKind::SurpriseReset);
    }

    #[test]
    fn rapid_repeat_reads_cannot_confirm_a_capacity_event() {
        let start = Utc::now();
        let old_reset = start + Duration::hours(4);
        let new_reset = start + Duration::hours(9);
        let mut observer = CapacityEventObserver::default();

        observer.observe(&snapshot(start, 85.0, old_reset));
        observer.observe(&snapshot(start + Duration::minutes(5), 10.0, new_reset));
        assert!(
            observer
                .observe(&snapshot(
                    start + Duration::minutes(5) + Duration::seconds(10),
                    11.0,
                    new_reset + Duration::minutes(1),
                ))
                .is_empty(),
            "back-to-back refreshes are not independent confirmation"
        );

        let events = observer.observe(&snapshot(
            start + Duration::minutes(5) + Duration::seconds(31),
            12.0,
            new_reset + Duration::minutes(2),
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, CapacityEventKind::SurpriseReset);
    }

    #[test]
    fn reset_time_jitter_and_small_usage_drops_do_not_emit() {
        let start = Utc::now();
        let reset = start + Duration::hours(4);
        let mut observer = CapacityEventObserver::default();

        observer.observe(&snapshot(start, 60.0, reset));
        assert!(
            observer
                .observe(&snapshot(
                    start + Duration::minutes(5),
                    50.0,
                    reset + Duration::minutes(5),
                ))
                .is_empty()
        );
    }

    #[test]
    fn partial_reset_with_unchanged_reset_time_requires_confirmation() {
        let start = Utc::now();
        let reset = start + Duration::days(22);
        let mut observer = CapacityEventObserver::default();

        observer.observe(&snapshot(start, 99.4, reset));
        assert!(
            observer
                .observe(&snapshot(start + Duration::minutes(5), 49.7, reset))
                .is_empty(),
            "a single provider read must not trigger a reset notification"
        );

        let events = observer.observe(&snapshot(
            start + Duration::minutes(10),
            49.7,
            reset + Duration::minutes(2),
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, CapacityEventKind::PartialReset);
        assert_eq!(events[0].previous_used_percent, 99.4);
        assert_eq!(events[0].current_used_percent, 49.7);
    }

    #[test]
    fn startup_at_a_partially_restored_value_does_not_replay_an_alert() {
        let start = Utc::now();
        let reset = start + Duration::days(22);
        let mut before_restart = CapacityEventObserver::default();
        before_restart.observe(&snapshot(start, 99.4, reset));

        let persisted = serde_json::to_string(&before_restart).unwrap();
        let mut after_restart: CapacityEventObserver = serde_json::from_str(&persisted).unwrap();
        assert!(
            after_restart
                .observe(&snapshot(start + Duration::minutes(5), 49.7, reset))
                .is_empty()
        );
        assert!(
            after_restart
                .observe(&snapshot(start + Duration::minutes(10), 49.7, reset))
                .is_empty()
        );
    }

    #[test]
    fn scheduled_reset_emits_on_the_first_post_reset_read() {
        let start = Utc::now();
        let old_reset = start + Duration::minutes(5);
        let new_reset = start + Duration::hours(5);
        let mut observer = CapacityEventObserver::default();

        observer.observe(&snapshot(start, 88.0, old_reset));
        let events = observer.observe(&snapshot(start + Duration::minutes(6), 3.0, new_reset));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, CapacityEventKind::ScheduledReset);
    }

    #[test]
    fn codex_weekly_reset_is_not_hidden_by_spark_weekly() {
        let start = Utc::now();
        let old_reset = start + Duration::minutes(5);
        let new_reset = start + Duration::days(7);
        let spark_reset = start + Duration::days(6);
        let mut observer = CapacityEventObserver::default();

        let mut before = with_extra(
            snapshot(start, 85.0, old_reset),
            "codex-spark-weekly",
            "Codex Spark Weekly",
            0.0,
            spark_reset,
        );
        before.primary_label = Some("Weekly".into());
        before.primary.window_minutes = Some(10_080);
        before.extra_rate_windows[0].window.window_minutes = Some(10_080);
        assert!(observer.observe(&before).is_empty());

        let mut after = with_extra(
            snapshot(start + Duration::minutes(6), 0.0, new_reset),
            "codex-spark-weekly",
            "Codex Spark Weekly",
            0.0,
            spark_reset,
        );
        after.primary_label = Some("Weekly".into());
        after.primary.window_minutes = Some(10_080);
        after.extra_rate_windows[0].window.window_minutes = Some(10_080);

        let events = observer.observe(&after);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, CapacityEventKind::ScheduledReset);
        assert_eq!(events[0].window_id, "weekly");
        assert_eq!(events[0].window_label, "Weekly");
    }

    #[test]
    fn observations_are_isolated_by_account_and_source() {
        let start = Utc::now();
        let reset = start + Duration::hours(4);
        let mut observer = CapacityEventObserver::default();
        observer.observe(&snapshot(start, 90.0, reset));

        let mut other = snapshot(
            start + Duration::minutes(5),
            5.0,
            reset + Duration::hours(5),
        );
        other.account_email = Some("other@example.com".into());
        assert!(observer.observe(&other).is_empty());
    }

    #[test]
    fn reset_time_shift_requires_confirmation() {
        let start = Utc::now();
        let reset = start + Duration::hours(4);
        let shifted = reset + Duration::hours(2);
        let mut observer = CapacityEventObserver::default();

        observer.observe(&snapshot(start, 40.0, reset));
        assert!(
            observer
                .observe(&snapshot(start + Duration::minutes(5), 42.0, shifted))
                .is_empty()
        );
        let events = observer.observe(&snapshot(
            start + Duration::minutes(10),
            43.0,
            shifted + Duration::minutes(2),
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, CapacityEventKind::ResetTimeShift);
    }

    #[test]
    fn lifted_window_requires_confirmation() {
        let start = Utc::now();
        let reset = start + Duration::hours(4);
        let mut observer = CapacityEventObserver::default();
        observer.observe(&with_extra(
            snapshot(start, 30.0, reset),
            "codex-weekly",
            "Weekly",
            70.0,
            reset,
        ));

        let lifted = with_inactive(
            snapshot(start + Duration::minutes(5), 31.0, reset),
            "codex-weekly",
            "Weekly",
        );
        assert!(observer.observe(&lifted).is_empty());
        let events = observer.observe(&with_inactive(
            snapshot(start + Duration::minutes(10), 32.0, reset),
            "codex-weekly",
            "Weekly",
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, CapacityEventKind::WindowLifted);
    }

    #[test]
    fn restored_window_and_new_allowance_are_distinguished() {
        let start = Utc::now();
        let reset = start + Duration::hours(4);
        let mut restored_observer = CapacityEventObserver::default();
        restored_observer.observe(&with_inactive(
            snapshot(start, 30.0, reset),
            "codex-weekly",
            "Weekly",
        ));
        restored_observer.observe(&with_extra(
            snapshot(start + Duration::minutes(5), 31.0, reset),
            "codex-weekly",
            "Weekly",
            10.0,
            reset,
        ));
        let restored = restored_observer.observe(&with_extra(
            snapshot(start + Duration::minutes(10), 32.0, reset),
            "codex-weekly",
            "Weekly",
            11.0,
            reset + Duration::minutes(2),
        ));
        assert_eq!(restored[0].kind, CapacityEventKind::WindowRestored);

        let mut allowance_observer = CapacityEventObserver::default();
        allowance_observer.observe(&snapshot(start, 30.0, reset));
        allowance_observer.observe(&with_extra(
            snapshot(start + Duration::minutes(5), 31.0, reset),
            "bonus",
            "Bonus",
            5.0,
            reset,
        ));
        let allowance = allowance_observer.observe(&with_extra(
            snapshot(start + Duration::minutes(10), 32.0, reset),
            "bonus",
            "Bonus",
            6.0,
            reset + Duration::minutes(2),
        ));
        assert_eq!(allowance[0].kind, CapacityEventKind::AllowanceGranted);
    }

    #[test]
    fn cursor_promotional_and_on_demand_pools_never_emit() {
        let start = Utc::now();
        let reset = start + Duration::hours(4);
        let mut observer = CapacityEventObserver::default();
        let mut baseline = snapshot(start, 30.0, reset);
        baseline.provider_id = "cursor".into();
        baseline.display_name = "Cursor".into();
        observer.observe(&baseline);

        for (id, title) in [
            ("cursor-promotional", "Promotional"),
            ("cursor-on-demand", "On-demand"),
        ] {
            let mut first = with_extra(
                snapshot(start + Duration::minutes(5), 31.0, reset),
                id,
                title,
                0.0,
                reset,
            );
            first.provider_id = "cursor".into();
            first.display_name = "Cursor".into();
            assert!(observer.observe(&first).is_empty());

            let mut second = with_extra(
                snapshot(start + Duration::minutes(10), 32.0, reset),
                id,
                title,
                0.0,
                reset,
            );
            second.provider_id = "cursor".into();
            second.display_name = "Cursor".into();
            assert!(observer.observe(&second).is_empty());
        }
    }

    #[test]
    fn restart_rebaselines_without_replaying_persisted_history() {
        let start = Utc::now();
        let old_reset = start + Duration::hours(4);
        let new_reset = start + Duration::hours(9);
        let mut before_restart = CapacityEventObserver::default();

        before_restart.observe(&snapshot(start, 85.0, old_reset));
        // Leave a surprise-reset candidate awaiting confirmation.
        before_restart.observe(&snapshot(start + Duration::minutes(5), 10.0, new_reset));

        let persisted = serde_json::to_string(&before_restart).unwrap();
        assert!(!persisted.contains("pending_resets"));
        assert!(!persisted.contains("pending_transitions"));

        let mut after_restart: CapacityEventObserver = serde_json::from_str(&persisted).unwrap();
        assert!(
            after_restart
                .observe(&snapshot(
                    start + Duration::minutes(10),
                    11.0,
                    new_reset + Duration::minutes(1),
                ))
                .is_empty(),
            "the first live reading after restart replaces persisted history"
        );
        assert!(
            after_restart
                .observe(&snapshot(
                    start + Duration::minutes(15),
                    12.0,
                    new_reset + Duration::minutes(2),
                ))
                .is_empty(),
            "an old pre-restart candidate must never be confirmed later"
        );
    }
}
