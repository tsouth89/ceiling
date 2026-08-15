//! Provider incident badges (SBS-280).
//!
//! Reads public provider status pages so a "0 tokens left" moment can be told
//! apart from an outage. This is the only outbound request Ceiling makes that
//! is not to a provider the user already signed in to, so it is opt-in and
//! cached hard: at most one request per provider per [`INCIDENT_TTL`], and only
//! while a surface is actually asking.
//!
//! Nothing about the user is sent. A status page request carries no
//! credentials, no account id, and no usage figures.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use codexbar::settings::Settings;
use codexbar::status::{StatusLevel, fetch_provider_status, get_status_page_url};
use serde::{Deserialize, Serialize};

/// How long a reading stays good. Status pages move on the order of minutes,
/// and a badge that is a quarter-hour stale is still far better than none.
const INCIDENT_TTL: Duration = Duration::from_secs(15 * 60);

/// Backoff after a failed poll, so an unreachable page is not retried on every
/// surface open. Shorter than the success TTL because a failure carries no
/// information.
const INCIDENT_ERROR_TTL: Duration = Duration::from_secs(5 * 60);

/// A provider's current public status, as shown on its badge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderIncident {
    pub provider_id: String,
    /// `degraded`, `partial`, or `major`. Operational providers are omitted
    /// entirely rather than sent as a green badge nobody needs.
    pub severity: String,
    /// The status page's own words, so Ceiling never paraphrases an incident.
    pub description: String,
    /// Status page the description came from, for the badge's link.
    pub status_page_url: String,
}

struct CachedIncident {
    loaded_at: Instant,
    /// `None` once a provider reads operational, which is still a fresh answer.
    incident: Option<ProviderIncident>,
    ok: bool,
}

impl CachedIncident {
    fn is_fresh(&self) -> bool {
        let ttl = if self.ok {
            INCIDENT_TTL
        } else {
            INCIDENT_ERROR_TTL
        };
        self.loaded_at.elapsed() < ttl
    }
}

fn cache() -> &'static Mutex<HashMap<String, CachedIncident>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedIncident>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Drop every reading. Called when the feature is switched off so re-enabling
/// shows current state rather than a badge from an hour ago.
pub fn clear() {
    if let Ok(mut guard) = cache().lock() {
        guard.clear();
    }
}

/// Only levels worth interrupting someone over. `Unknown` is deliberately not
/// a badge: a status page that failed to parse is not evidence of an outage,
/// and showing one would make the badge untrustworthy exactly when it matters.
fn severity_label(level: StatusLevel) -> Option<&'static str> {
    match level {
        StatusLevel::Degraded => Some("degraded"),
        StatusLevel::Partial => Some("partial"),
        StatusLevel::Major => Some("major"),
        StatusLevel::Operational | StatusLevel::Unknown => None,
    }
}

/// Providers whose status pages we can read, among those the user enabled.
pub fn pollable_provider_ids(settings: &Settings) -> Vec<String> {
    if !settings.provider_incident_badges_enabled {
        return Vec::new();
    }
    let mut ids: Vec<String> = settings
        .get_enabled_provider_ids()
        .into_iter()
        .map(|provider| provider.cli_name().to_string())
        .filter(|provider_id| get_status_page_url(provider_id).is_some())
        .collect();
    // Several enabled accounts of one provider share a status page.
    ids.sort();
    ids.dedup();
    ids
}

/// Cached incidents, refreshing any provider whose reading has aged out.
///
/// Providers that are operational, unreadable, or have no public status page
/// are simply absent from the map.
pub async fn current_incidents(settings: &Settings) -> HashMap<String, ProviderIncident> {
    let provider_ids = pollable_provider_ids(settings);
    if provider_ids.is_empty() {
        clear();
        return HashMap::new();
    }

    let stale: Vec<String> = {
        let Ok(guard) = cache().lock() else {
            return HashMap::new();
        };
        provider_ids
            .iter()
            .filter(|id| guard.get(*id).is_none_or(|entry| !entry.is_fresh()))
            .cloned()
            .collect()
    };

    if !stale.is_empty() {
        // Concurrent rather than sequential: each page carries its own ten
        // second timeout, so a serial loop over a handful of providers could
        // keep a surface waiting for the better part of a minute.
        let tasks: Vec<_> = stale
            .into_iter()
            .map(|provider_id| {
                tauri::async_runtime::spawn(async move {
                    let status = fetch_provider_status(&provider_id).await;
                    (provider_id, status)
                })
            })
            .collect();
        let mut fetched = Vec::with_capacity(tasks.len());
        for task in tasks {
            match task.await {
                Ok(result) => fetched.push(result),
                Err(error) => tracing::warn!("provider status poll failed: {error}"),
            }
        }
        if let Ok(mut guard) = cache().lock() {
            for (provider_id, status) in fetched {
                let ok = status.is_some();
                let incident = status.and_then(|status| {
                    let severity = severity_label(status.level)?;
                    Some(ProviderIncident {
                        severity: severity.to_string(),
                        description: status.description,
                        status_page_url: get_status_page_url(&provider_id)
                            .unwrap_or_default()
                            .to_string(),
                        provider_id: provider_id.clone(),
                    })
                });
                guard.insert(
                    provider_id,
                    CachedIncident {
                        loaded_at: Instant::now(),
                        incident,
                        ok,
                    },
                );
            }
        }
    }

    let Ok(guard) = cache().lock() else {
        return HashMap::new();
    };
    provider_ids
        .into_iter()
        .filter_map(|provider_id| {
            let incident = guard.get(&provider_id)?.incident.clone()?;
            Some((provider_id, incident))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_incidents_earn_a_badge() {
        assert_eq!(severity_label(StatusLevel::Degraded), Some("degraded"));
        assert_eq!(severity_label(StatusLevel::Partial), Some("partial"));
        assert_eq!(severity_label(StatusLevel::Major), Some("major"));
        assert_eq!(severity_label(StatusLevel::Operational), None);
        // A page we could not parse is not evidence of an outage.
        assert_eq!(severity_label(StatusLevel::Unknown), None);
    }

    #[test]
    fn nothing_is_polled_while_the_feature_is_off() {
        let settings = Settings {
            enabled_providers: ["codex".to_string(), "claude".to_string()]
                .into_iter()
                .collect(),
            ..Settings::default()
        };
        assert!(pollable_provider_ids(&settings).is_empty());
    }

    #[test]
    fn only_enabled_providers_with_a_status_page_are_polled() {
        let settings = Settings {
            provider_incident_badges_enabled: true,
            // `zai` has no public status page; it must not produce a request.
            enabled_providers: ["claude".to_string(), "zai".to_string(), "codex".to_string()]
                .into_iter()
                .collect(),
            ..Settings::default()
        };

        assert_eq!(
            pollable_provider_ids(&settings),
            vec!["claude".to_string(), "codex".to_string()],
        );
    }

    #[test]
    fn a_failed_poll_backs_off_for_less_time_than_a_good_one() {
        let ok = CachedIncident {
            loaded_at: Instant::now() - Duration::from_secs(6 * 60),
            incident: None,
            ok: true,
        };
        let failed = CachedIncident {
            loaded_at: Instant::now() - Duration::from_secs(6 * 60),
            incident: None,
            ok: false,
        };

        assert!(ok.is_fresh(), "a good reading is still valid at 6 minutes");
        assert!(!failed.is_fresh(), "a failure is retried sooner");
    }
}
