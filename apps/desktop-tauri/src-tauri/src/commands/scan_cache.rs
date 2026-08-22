//! Disk-backed stale-while-revalidate cache for local transcript scans.
//!
//! The cards that read local logs (Estimated API value, the activity heatmap)
//! each cost a full pass over gigabytes of Codex/Claude transcripts. Without a
//! cache every mount pays that again, so switching tabs re-ran a scan the user
//! had already waited through once.
//!
//! This keeps the last result per key, in memory and on disk, and serves it
//! immediately while a refresh runs behind it. The provider chart cache in
//! `chart.rs` predates this and follows the same shape; it is not folded in
//! here because its entries carry a live reset window and need eviction rules
//! of their own.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// One cached scan result and when it was produced.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub(crate) struct CachedScan<T> {
    pub(crate) refreshed_at_ms: i64,
    pub(crate) value: T,
}

#[derive(Debug, Serialize, serde::Deserialize)]
struct PersistedScanCache<T> {
    #[serde(default)]
    version: u8,
    /// Fingerprint of the model prices these results were computed under.
    ///
    /// Both cards carry dollar figures, so this is derived data rather than a
    /// user setting: a file written under different prices is discarded, the
    /// same way the record index is. Without it a cache written minutes before
    /// a price change would keep serving the old dollars for its whole TTL,
    /// and the index rebuild behind it would never be asked for.
    #[serde(default)]
    fingerprint: u64,
    #[serde(default = "HashMap::new")]
    entries: HashMap<String, CachedScan<T>>,
}

impl<T> Default for PersistedScanCache<T> {
    fn default() -> Self {
        Self {
            version: 0,
            fingerprint: 0,
            entries: HashMap::new(),
        }
    }
}

/// Entries kept per cache file. Keys carry the local calendar date, so a
/// machine left running for weeks would otherwise accumulate one dead entry per
/// day. Oldest refresh wins the eviction.
const MAX_ENTRIES: usize = 8;

pub(crate) struct ScanCache<T: 'static> {
    file_name: &'static str,
    /// Bumped when the cached shape changes, so an older file is discarded
    /// rather than deserialized into today's struct.
    version: u8,
    state: OnceLock<Mutex<PersistedScanCache<T>>>,
    refreshing: OnceLock<Mutex<HashSet<String>>>,
    /// One gate per key for the cold path, so two callers arriving on an empty
    /// cache do not both start the same multi-gigabyte scan. Launch prewarming
    /// and a card opened straight away is exactly that race.
    building: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl<T> ScanCache<T>
where
    T: Clone + Send + Serialize + DeserializeOwned + 'static,
{
    pub(crate) const fn new(file_name: &'static str, version: u8) -> Self {
        Self {
            file_name,
            version,
            state: OnceLock::new(),
            refreshing: OnceLock::new(),
            building: OnceLock::new(),
        }
    }

    /// The cached value for `key`, rebuilding it only when nothing is cached.
    ///
    /// A cached entry older than `ttl` is still returned, with a refresh
    /// scheduled behind it: a stale number that paints instantly beats a
    /// spinner over a scan the user already waited through. `build` runs on a
    /// blocking worker; a worker that dies surfaces as `Err(error_message)`
    /// rather than an empty result, because "unavailable" and "no local
    /// activity" are different answers on these cards.
    pub(crate) async fn load<F, N>(
        &'static self,
        key: String,
        ttl: Duration,
        build: F,
        on_refreshed: N,
        error_message: &str,
    ) -> Result<T, String>
    where
        F: Fn() -> T + Send + Sync + Clone + 'static,
        N: Fn() + Send + 'static,
    {
        if let Some(cached) = self.cached(&key) {
            if is_stale(cached.refreshed_at_ms, now_ms(), ttl) {
                self.schedule_refresh(key, build, on_refreshed);
            }
            return Ok(cached.value);
        }

        // Hold the per-key gate across the whole cold build. A second caller
        // waits here and then finds the value below rather than starting its
        // own scan of the same gigabytes.
        let gate = self.gate(&key);
        let _building = gate.lock().await;
        if let Some(cached) = self.cached(&key) {
            return Ok(cached.value);
        }

        let (started, value) = tauri::async_runtime::spawn_blocking(move || {
            // Capture before the build, not after: a catalog refresh that
            // lands during the scan is its own state (SBS-946). Stamping the
            // new fingerprint on a value priced under the old one would serve
            // those dollars as current for the full TTL.
            let started = codexbar::usage_index::pricing_fingerprint();
            (started, build())
        })
        .await
        .map_err(|err| {
            tracing::warn!("{} scan worker failed: {}", self.file_name, err);
            error_message.to_string()
        })?;
        self.store(key, value.clone(), started);
        Ok(value)
    }

    /// The gate for one key, created on first use.
    fn gate(&'static self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut guard = self
            .building()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.entry(key.to_string()).or_default().clone()
    }

    fn cached(&'static self, key: &str) -> Option<CachedScan<T>> {
        let mut guard = self.entries().lock().ok()?;
        // Prices moved since this was written, so the dollars in it are stale
        // in a way the TTL knows nothing about.
        if guard.fingerprint != codexbar::usage_index::pricing_fingerprint() {
            if !guard.entries.is_empty() {
                tracing::info!("model prices changed; dropping cached {}", self.file_name);
                guard.entries.clear();
            }
            return None;
        }
        guard.entries.get(key).cloned()
    }

    /// Whether anything has ever been cached here, for any key.
    ///
    /// Used as evidence that the user opens the surface this cache belongs to:
    /// nothing writes these files until a card is opened at least once.
    pub(crate) fn has_entries(&'static self) -> bool {
        self.entries()
            .lock()
            .is_ok_and(|guard| !guard.entries.is_empty())
    }

    fn store(&'static self, key: String, value: T, started: u64) {
        let Ok(mut guard) = self.entries().lock() else {
            return;
        };
        let current = codexbar::usage_index::pricing_fingerprint();
        if !apply_store(
            &mut guard,
            self.version,
            started,
            current,
            key,
            value,
            now_ms(),
        ) {
            tracing::info!(
                "model prices changed mid-scan; dropping cached {} write",
                self.file_name
            );
            return;
        }
        if let Some(path) = self.path() {
            write_cache(&path, &*guard);
        }
    }

    fn schedule_refresh<F, N>(&'static self, key: String, build: F, on_refreshed: N)
    where
        F: Fn() -> T + Send + Sync + Clone + 'static,
        N: Fn() + Send + 'static,
    {
        let Ok(mut active) = self.refreshing().lock() else {
            return;
        };
        if !active.insert(key.clone()) {
            return;
        }
        drop(active);

        tauri::async_runtime::spawn(async move {
            match tauri::async_runtime::spawn_blocking(move || {
                let started = codexbar::usage_index::pricing_fingerprint();
                (started, build())
            })
            .await
            {
                Ok((started, value)) => {
                    self.store(key.clone(), value, started);
                    // A card that is already open asked before this ran and got
                    // the stale answer. Without this it would keep showing it
                    // until it was remounted.
                    on_refreshed();
                }
                Err(err) => tracing::warn!("{} refresh failed: {}", self.file_name, err),
            }
            if let Ok(mut active) = self.refreshing().lock() {
                active.remove(&key);
            }
        });
    }

    fn entries(&'static self) -> &'static Mutex<PersistedScanCache<T>> {
        self.state.get_or_init(|| Mutex::new(self.load_from_disk()))
    }

    fn refreshing(&'static self) -> &'static Mutex<HashSet<String>> {
        self.refreshing.get_or_init(|| Mutex::new(HashSet::new()))
    }

    fn building(&'static self) -> &'static Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
        self.building.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn path(&self) -> Option<PathBuf> {
        codexbar::settings::Settings::settings_path()
            .and_then(|path| path.parent().map(|parent| parent.join(self.file_name)))
    }

    fn load_from_disk(&self) -> PersistedScanCache<T> {
        let Some(path) = self.path() else {
            return PersistedScanCache::default();
        };
        let prices = codexbar::usage_index::pricing_fingerprint();
        fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PersistedScanCache<T>>(&bytes).ok())
            .filter(|cache| cache.version == self.version && cache.fingerprint == prices)
            .unwrap_or_default()
    }
}

/// Whether a cached entry is old enough to schedule a refresh behind it.
///
/// An entry stamped in the future (a clock that moved backwards) counts as
/// fresh rather than as stale forever: `saturating_sub` floors the age at zero.
fn is_stale(refreshed_at_ms: i64, now_ms: i64, ttl: Duration) -> bool {
    now_ms.saturating_sub(refreshed_at_ms) > ttl.as_millis() as i64
}

/// Evict the least recently refreshed entries until the map fits. `keep` is the
/// key the caller just stored, which a backwards clock could otherwise select.
fn prune<T>(cache: &mut PersistedScanCache<T>, keep: &str) {
    if cache.entries.len() <= MAX_ENTRIES {
        return;
    }
    let mut by_age: Vec<(String, i64)> = cache
        .entries
        .iter()
        .filter(|(key, _)| key.as_str() != keep)
        .map(|(key, entry)| (key.clone(), entry.refreshed_at_ms))
        .collect();
    // Oldest first; the key breaks ties so eviction does not depend on HashMap
    // iteration order.
    by_age.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    for (key, _) in by_age.into_iter().take(cache.entries.len() - MAX_ENTRIES) {
        cache.entries.remove(&key);
    }
}

/// Persist a built scan only when the prices it started under are still
/// the prices in force.
///
/// A mid-build catalog refresh is its own state: the value was computed
/// under `started`. Writing it under `current` would label old dollars as
/// current for every key already in the file, not just this one. The
/// write is dropped rather than relabelled. Zero is the "never captured"
/// default, not a hash of today's rates.
fn apply_store<T>(
    cache: &mut PersistedScanCache<T>,
    version: u8,
    started: u64,
    current: u64,
    key: String,
    value: T,
    now_ms: i64,
) -> bool {
    if started == 0 || started != current {
        return false;
    }
    // The entries already in the file were priced under whatever fingerprint
    // it carries. Stamping it with `started` while keeping them would relabel
    // every other key's value as current — the heatmap's dates, or the
    // Estimated API value card's other range — and they would be served that
    // way until the TTL or a rewrite. `cached()` drops them on read for
    // exactly this reason; a write has to do the same, or one key's scan
    // launders another key's stale dollars (SBS-946).
    if (cache.fingerprint != started || cache.version != version) && !cache.entries.is_empty() {
        tracing::info!(
            cached = cache.entries.len(),
            "model prices or the cache layout moved; dropping cached entries rather than relabelling them"
        );
        cache.entries.clear();
    }
    cache.version = version;
    cache.fingerprint = started;
    cache.entries.insert(
        key.clone(),
        CachedScan {
            refreshed_at_ms: now_ms,
            value,
        },
    );
    prune(cache, &key);
    true
}

fn write_cache<T: Serialize>(path: &std::path::Path, cache: &PersistedScanCache<T>) {
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        tracing::warn!("failed to create scan cache directory: {error}");
        return;
    }
    match serde_json::to_vec(cache) {
        Ok(bytes) => {
            if let Err(error) = codexbar::secure_file::atomic_write(path, &bytes) {
                tracing::warn!("failed to persist scan cache: {error}");
            }
        }
        Err(error) => tracing::warn!("failed to serialize scan cache: {error}"),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Row(u32);

    fn cache_with(entries: &[(&str, i64)]) -> PersistedScanCache<Row> {
        let mut cache = PersistedScanCache::<Row>::default();
        for (index, (key, refreshed_at_ms)) in entries.iter().enumerate() {
            cache.entries.insert(
                (*key).to_string(),
                CachedScan {
                    refreshed_at_ms: *refreshed_at_ms,
                    value: Row(index as u32),
                },
            );
        }
        cache
    }

    #[test]
    fn prune_keeps_the_most_recent_entries() {
        let entries: Vec<(String, i64)> = (0..MAX_ENTRIES + 3)
            .map(|index| (format!("key-{index}"), index as i64))
            .collect();
        let borrowed: Vec<(&str, i64)> = entries
            .iter()
            .map(|(key, age)| (key.as_str(), *age))
            .collect();
        let mut cache = cache_with(&borrowed);

        prune(&mut cache, "key-0");

        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        // The freshly stored key survives even though it is the oldest.
        assert!(cache.entries.contains_key("key-0"));
        assert!(!cache.entries.contains_key("key-1"));
        assert!(
            cache
                .entries
                .contains_key(&format!("key-{}", MAX_ENTRIES + 2))
        );
    }

    #[test]
    fn an_entry_is_served_until_the_ttl_lapses() {
        let ttl = Duration::from_secs(300);
        let now = 1_000_000_000;

        assert!(!is_stale(now, now, ttl));
        assert!(!is_stale(now - 299_000, now, ttl));
        assert!(!is_stale(now - 300_000, now, ttl));
        assert!(is_stale(now - 300_001, now, ttl));
    }

    #[test]
    fn a_clock_that_moved_backwards_does_not_make_an_entry_stale() {
        // Stamped a minute into the future. The age floors at zero rather than
        // going negative and tipping the comparison the wrong way.
        let ttl = Duration::from_secs(300);
        let now = 1_000_000_000;

        assert!(!is_stale(now + 60_000, now, ttl));
    }

    /// SBS-946: the other direction. A scan that starts *after* the catalog
    /// moved passes `started == current`, so the write is allowed - but the
    /// file on disk is still stamped F1 and still holds F1-priced values for
    /// other keys. Stamping it F2 and keeping them served old dollars as
    /// current until the TTL. The stale entries go instead.
    #[test]
    fn a_write_under_new_prices_drops_the_entries_priced_under_the_old_ones() {
        let mut cache = cache_with(&[("today", 1), ("yesterday", 2)]);
        cache.version = 1;
        cache.fingerprint = 7;

        let stored = apply_store(&mut cache, 1, 8, 8, "tomorrow".to_string(), Row(99), 2);

        assert!(stored, "a scan wholly under the new prices may be written");
        assert_eq!(
            cache.fingerprint, 8,
            "the file now describes the new prices"
        );
        assert!(
            !cache.entries.contains_key("today") && !cache.entries.contains_key("yesterday"),
            "values priced under the old catalog must not be relabelled as current"
        );
        assert!(
            cache.entries.contains_key("tomorrow"),
            "the value actually priced under the new catalog stays"
        );
    }

    /// A cache already on the current fingerprint keeps its other keys: this
    /// must not turn every write into a cache flush.
    #[test]
    fn a_write_under_the_same_prices_keeps_the_other_keys() {
        let mut cache = cache_with(&[("today", 1)]);
        cache.version = 1;
        cache.fingerprint = 8;

        let stored = apply_store(&mut cache, 1, 8, 8, "tomorrow".to_string(), Row(99), 2);

        assert!(stored);
        assert!(
            cache.entries.contains_key("today"),
            "an entry priced under the same catalog is still current"
        );
        assert!(cache.entries.contains_key("tomorrow"));
    }

    /// A value priced under F1 must not relabel the whole cache as F2.
    ///
    /// `store` used to write `pricing_fingerprint()` at persist time, so a
    /// build that straddled a catalog refresh stamped old dollars as current
    /// for every key already in the file (SBS-946).
    #[test]
    fn a_mid_build_price_change_is_not_stamped_as_current() {
        let mut cache = cache_with(&[("today", 1)]);
        cache.version = 1;
        cache.fingerprint = 7;

        let stored = apply_store(&mut cache, 1, 7, 8, "tomorrow".to_string(), Row(99), 2);

        assert!(!stored, "a mid-build price change must drop the write");
        assert_eq!(
            cache.fingerprint, 7,
            "existing keys must keep the old stamp"
        );
        assert!(cache.entries.contains_key("today"));
        assert!(
            !cache.entries.contains_key("tomorrow"),
            "F1-priced value must not land under an F2 stamp"
        );
    }

    #[test]
    fn a_build_that_finishes_under_the_same_prices_is_stored() {
        let mut cache = cache_with(&[("today", 1)]);
        cache.version = 1;
        cache.fingerprint = 7;

        let stored = apply_store(&mut cache, 1, 7, 7, "tomorrow".to_string(), Row(99), 2);

        assert!(stored);
        assert_eq!(cache.fingerprint, 7);
        assert_eq!(
            cache.entries.get("tomorrow").map(|row| row.value.clone()),
            Some(Row(99))
        );
    }

    #[test]
    fn an_unknown_price_stamp_is_not_treated_as_current() {
        let mut cache = PersistedScanCache::<Row>::default();
        assert!(!apply_store(
            &mut cache,
            1,
            0,
            0,
            "today".to_string(),
            Row(1),
            1
        ));
        assert!(cache.entries.is_empty());
        assert_eq!(cache.fingerprint, 0);
    }

    /// A file written under other prices is not read back.
    ///
    /// These entries carry dollar figures, so a price change makes them wrong
    /// rather than merely old, and the TTL cannot see that.
    #[test]
    fn a_cache_written_under_other_prices_is_discarded() {
        let mut cache = cache_with(&[("today", 1)]);
        cache.version = 1;
        cache.fingerprint = 7;
        let bytes = serde_json::to_vec(&cache).expect("encode");

        let same: PersistedScanCache<Row> = serde_json::from_slice(&bytes).expect("decode");
        assert_eq!(same.entries.len(), 1);

        let reread: Option<PersistedScanCache<Row>> = serde_json::from_slice(&bytes)
            .ok()
            .filter(|cache: &PersistedScanCache<Row>| cache.version == 1 && cache.fingerprint == 8);
        assert!(
            reread.is_none(),
            "a different fingerprint must not be read back"
        );
    }

    #[test]
    fn prune_leaves_a_cache_within_bounds_alone() {
        let mut cache = cache_with(&[("a", 1), ("b", 2)]);

        prune(&mut cache, "a");

        assert_eq!(cache.entries.len(), 2);
    }
}
