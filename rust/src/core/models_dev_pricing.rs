#[cfg(test)]
mod tests {
    use super::{
        ModelsDevCache, ModelsDevCacheArtifact, ModelsDevCatalog, ModelsDevRefreshCoordinator,
        fingerprint_catalog_at, fingerprint_catalog_prices,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    /// Two providers with two models each, written in a caller-chosen key
    /// order. `ModelsDevCatalog` stores both levels in `HashMap`s, so the
    /// order the JSON arrives in is the order the maps iterate in.
    fn catalog_two_providers(input_per_million: f64, reversed: bool) -> ModelsDevCatalog {
        let openai = format!(
            r#""openai": {{
                "id": "openai",
                "models": {{
                    "openai/gpt-fresh": {{
                        "id": "openai/gpt-fresh",
                        "cost": {{ "input": {input_per_million}, "output": 10 }}
                    }},
                    "openai/gpt-mini": {{
                        "id": "openai/gpt-mini",
                        "cost": {{ "input": 0.25, "output": 2 }}
                    }}
                }}
            }}"#
        );
        let anthropic = r#""anthropic": {
                "id": "anthropic",
                "models": {
                    "anthropic/claude-sonnet": {
                        "id": "anthropic/claude-sonnet",
                        "cost": { "input": 3, "output": 15 }
                    },
                    "anthropic/claude-haiku": {
                        "id": "anthropic/claude-haiku",
                        "cost": { "input": 1, "output": 5 }
                    }
                }
            }"#
        .to_string();
        let ordered = if reversed {
            format!("{{{anthropic}, {openai}}}")
        } else {
            format!("{{{openai}, {anthropic}}}")
        };
        ModelsDevCatalog::decode(&ordered).expect("catalog")
    }

    /// A one-model catalog, enough to tell two saves apart by lookup.
    fn catalog_priced_at(input_per_million: f64) -> ModelsDevCatalog {
        ModelsDevCatalog::decode(&format!(
            r#"{{
                "openai": {{
                    "id": "openai",
                    "models": {{
                        "openai/gpt-fresh": {{
                            "id": "openai/gpt-fresh",
                            "cost": {{ "input": {input_per_million}, "output": 10 }}
                        }}
                    }}
                }}
            }}"#
        ))
        .expect("catalog")
    }

    #[test]
    fn save_writes_a_complete_catalog_to_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = UNIX_EPOCH + Duration::from_secs(1_000);

        assert!(ModelsDevCache::save(
            catalog_priced_at(2.5),
            now,
            Some(dir.path())
        ));

        // Read the file rather than calling `load`. `load` consults CACHE_MEMO,
        // which `save` just populated in this process, so it would return the
        // in-memory artifact even if nothing reached disk.
        let path = ModelsDevCache::cache_path(Some(dir.path()));
        let on_disk: ModelsDevCacheArtifact =
            serde_json::from_slice(&std::fs::read(&path).expect("read the cache file"))
                .expect("the file itself must hold a complete catalog");
        assert_eq!(
            on_disk
                .catalog
                .lookup("openai", "gpt-fresh")
                .expect("pricing")
                .input_cost_per_token,
            2.5e-6
        );
        assert_eq!(on_disk.version, ModelsDevCache::ARTIFACT_VERSION);
    }

    /// SBS-941: a daily refetch rewrites `fetched_at` and reshuffles HashMap
    /// keys. The fingerprint that invalidates dollar caches must ignore both.
    #[test]
    fn price_fingerprint_ignores_fetch_time_and_key_order() {
        let first = catalog_priced_at(2.5);
        let second = catalog_priced_at(2.5);
        assert_eq!(
            fingerprint_catalog_prices(&first),
            fingerprint_catalog_prices(&second)
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let early = UNIX_EPOCH + Duration::from_secs(1_000);
        let later = UNIX_EPOCH + Duration::from_secs(1_000 + 24 * 60 * 60);
        assert!(ModelsDevCache::save(
            catalog_priced_at(2.5),
            early,
            Some(dir.path())
        ));
        let first_bytes = std::fs::read(ModelsDevCache::cache_path(Some(dir.path()))).unwrap();
        assert!(ModelsDevCache::save(
            catalog_priced_at(2.5),
            later,
            Some(dir.path())
        ));
        let second_bytes = std::fs::read(ModelsDevCache::cache_path(Some(dir.path()))).unwrap();
        assert_ne!(
            first_bytes, second_bytes,
            "a refetch must actually rewrite the artifact"
        );

        let first_artifact: ModelsDevCacheArtifact =
            serde_json::from_slice(&first_bytes).expect("first artifact");
        let second_artifact: ModelsDevCacheArtifact =
            serde_json::from_slice(&second_bytes).expect("second artifact");
        assert_eq!(
            fingerprint_catalog_prices(&first_artifact.catalog),
            fingerprint_catalog_prices(&second_artifact.catalog)
        );
        assert_ne!(
            fingerprint_catalog_prices(&catalog_priced_at(2.5)),
            fingerprint_catalog_prices(&catalog_priced_at(3.0))
        );
    }

    /// The sort inside the fingerprint is the only thing standing between a
    /// reshuffled `HashMap` and a daily cache wipe, so exercise both levels
    /// with more than one key.
    #[test]
    fn price_fingerprint_ignores_provider_and_model_key_order() {
        assert_eq!(
            fingerprint_catalog_prices(&catalog_two_providers(2.5, false)),
            fingerprint_catalog_prices(&catalog_two_providers(2.5, true))
        );
        assert_ne!(
            fingerprint_catalog_prices(&catalog_two_providers(2.5, false)),
            fingerprint_catalog_prices(&catalog_two_providers(3.0, true)),
            "a rate change still has to move the fingerprint"
        );
    }

    /// Identifiers are folded in one after another. Without a length frame,
    /// `("ab", "c")` and `("a", "bc")` hash the same, so a rename between two
    /// models could hide a rate change.
    #[test]
    fn price_fingerprint_frames_identifiers() {
        let split = |first: &str, second: &str| {
            ModelsDevCatalog::decode(&format!(
                r#"{{
                    "openai": {{
                        "id": "openai",
                        "models": {{
                            "{first}": {{
                                "id": "{second}",
                                "cost": {{ "input": 1, "output": 2 }}
                            }}
                        }}
                    }}
                }}"#
            ))
            .expect("catalog")
        };
        assert_ne!(
            fingerprint_catalog_prices(&split("ab", "c")),
            fingerprint_catalog_prices(&split("a", "bc"))
        );
    }

    /// SBS-941: `lookup` refuses a catalog past `CACHE_TTL`, so anything priced
    /// while it was stale used the built-in rate card. Hashing the stale file
    /// would make the refresh that revives the same rates a no-op, and the
    /// fallback dollars would survive the rescan that is supposed to fix them.
    #[test]
    fn price_fingerprint_treats_a_stale_catalog_as_no_catalog() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Some(dir.path());
        let fetched = UNIX_EPOCH + Duration::from_secs(1_000);
        let fresh = fetched + Duration::from_secs(60 * 60);
        let expired = fetched + Duration::from_secs(25 * 60 * 60);

        assert_eq!(
            fingerprint_catalog_at(fresh, root),
            0,
            "no catalog on disk hashes as no catalog"
        );

        assert!(ModelsDevCache::save(catalog_priced_at(2.5), fetched, root));
        let priced = fingerprint_catalog_at(fresh, root);
        assert_ne!(priced, 0, "a usable catalog has to hash to something");
        assert_eq!(
            fingerprint_catalog_at(expired, root),
            0,
            "a catalog `lookup` refuses must hash as no catalog"
        );

        // The daily refetch that finds the same rates: new `fetched_at`, same
        // prices. It must revive the fingerprint the fresh catalog had, so the
        // fallback-priced records written while it was stale get dropped.
        assert!(ModelsDevCache::save(catalog_priced_at(2.5), expired, root));
        assert_eq!(fingerprint_catalog_at(expired, root), priced);

        assert!(ModelsDevCache::save(catalog_priced_at(3.0), expired, root));
        assert_ne!(fingerprint_catalog_at(expired, root), priced);
    }

    /// SBS-870: `File::create` truncated the live cache to zero bytes before
    /// the new JSON landed. A second process reading in that window - or this
    /// one dying mid-write - lost a good catalog and reported no prices.
    ///
    /// This test holds a real, live handle open on the cache the way a
    /// concurrent reader mid-`load` would, then triggers a second `save`, and
    /// is not platform-gated: it must fail on `windows-latest` CI (where the
    /// unix-only inode check above never ran) if `save` regresses to
    /// `File::create` + `write_all`.
    ///
    /// Deliberately says nothing about whether the second `save` reports
    /// success. On Unix `rename` always wins over an open reader, but on
    /// Windows `MoveFileExW` racing a held handle is nondeterministic: ten
    /// repeats of this test on this machine gave failure, failure, then
    /// success. Asserting either outcome buys a flaky test. So this asserts
    /// only the property that holds under both outcomes and that a truncating
    /// write breaks:
    ///
    /// - replace won  -> the path points at the new file and this handle still
    ///   holds the whole old one
    /// - replace lost -> nothing was written, so the old file is untouched
    /// - `File::create` -> the file this handle is reading is truncated to
    ///   zero underneath it, and the parse below fails
    ///
    /// The Windows nondeterminism is worth knowing about beyond the test: a
    /// `save` there can genuinely fail while another process holds the cache
    /// open. It fails closed, keeping the previous catalog, and the next
    /// refresh retries.
    #[test]
    fn save_does_not_overwrite_the_cache_while_a_reader_holds_it_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let path = ModelsDevCache::cache_path(Some(dir.path()));

        assert!(ModelsDevCache::save(
            catalog_priced_at(2.5),
            now,
            Some(dir.path())
        ));

        // Hold the file open the way a concurrent reader mid-`load` would.
        let mut reader = std::fs::File::open(&path).expect("open the live cache");

        let _ = ModelsDevCache::save(catalog_priced_at(9.0), now, Some(dir.path()));

        let mut previous = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut previous).expect("read the held handle");
        let held = serde_json::from_slice::<ModelsDevCacheArtifact>(&previous)
            .expect("the held handle must still see a complete catalog");
        assert_eq!(
            held.catalog
                .lookup("openai", "gpt-fresh")
                .expect("pricing")
                .input_cost_per_token,
            2.5e-6,
            "the concurrent reader must still get the pre-save catalog, not a \
             truncated or half-rewritten one"
        );
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let path = ModelsDevCache::cache_path(Some(dir.path()));

        assert!(ModelsDevCache::save(
            catalog_priced_at(2.5),
            now,
            Some(dir.path())
        ));

        let parent = path.parent().expect("cache dir");
        let stray: Vec<_> = std::fs::read_dir(parent)
            .expect("read cache dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| name != "models-dev-v1.json")
            .collect();
        assert!(stray.is_empty(), "left temp files behind: {stray:?}");
    }

    #[test]
    fn decodes_top_level_provider_map_and_converts_million_token_rates() {
        let catalog = ModelsDevCatalog::decode(
            r#"{
                "openai": {
                    "id": "openai",
                    "models": {
                        "openai/gpt-fresh": {
                            "id": "openai/gpt-fresh",
                            "cost": {
                                "input": 2.5,
                                "output": 10,
                                "cache_read": 0.25,
                                "cache_write": 3.75,
                                "context_over_200k": {
                                    "input": 5,
                                    "output": 15,
                                    "cache_read": 0.5,
                                    "cache_write": 7.5
                                }
                            }
                        }
                    }
                },
                "anthropic": {
                    "models": {
                        "claude-fresh": {
                            "id": "claude-fresh",
                            "cost": { "input": 3, "output": 15 }
                        }
                    }
                }
            }"#,
        )
        .expect("top-level catalog");

        let pricing = catalog.lookup("openai", "gpt-fresh").expect("pricing");
        assert_eq!(pricing.input_cost_per_token, 2.5e-6);
        assert_eq!(pricing.output_cost_per_token, 10e-6);
        assert_eq!(pricing.cache_read_input_cost_per_token, Some(0.25e-6));
        assert_eq!(pricing.cache_write_input_cost_per_token, Some(3.75e-6));
        assert_eq!(pricing.threshold_tokens, Some(200_000));
        assert_eq!(pricing.input_cost_per_token_above_threshold, Some(5e-6));
    }

    #[test]
    fn decodes_providers_envelope() {
        let catalog = ModelsDevCatalog::decode(
            r#"{
                "providers": {
                    "anthropic": {
                        "id": "anthropic",
                        "models": {
                            "claude-fresh": {
                                "id": "claude-fresh",
                                "cost": { "input": 3, "output": 15 }
                            }
                        }
                    },
                    "openai": {
                        "models": {
                            "gpt-fresh": {
                                "id": "gpt-fresh",
                                "cost": { "input": 2.5, "output": 10 }
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("enveloped catalog");

        assert_eq!(
            catalog
                .lookup("anthropic", "claude-fresh")
                .expect("pricing")
                .output_cost_per_token,
            15e-6
        );
    }

    #[test]
    fn cache_artifact_is_versioned_and_expires_after_one_day() {
        let catalog = ModelsDevCatalog::decode(
            r#"{
                "openai": {
                    "models": {
                        "gpt-fresh": {
                            "id": "gpt-fresh",
                            "cost": { "input": 2.5, "output": 10 }
                        }
                    }
                },
                "anthropic": {
                    "models": {
                        "claude-fresh": {
                            "id": "claude-fresh",
                            "cost": { "input": 3, "output": 15 }
                        }
                    }
                }
            }"#,
        )
        .expect("catalog");
        let fetched_at = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let artifact = ModelsDevCacheArtifact::new(catalog, fetched_at);

        assert_eq!(artifact.version, ModelsDevCache::ARTIFACT_VERSION);
        assert!(!artifact.is_stale(fetched_at + Duration::from_secs(86_400)));
        assert!(artifact.is_stale(fetched_at + Duration::from_secs(86_401)));
        assert_eq!(
            ModelsDevCache::cache_path(Some(PathBuf::from("cache-root").as_path())),
            PathBuf::from("cache-root")
                .join("model-pricing")
                .join("models-dev-v1.json")
        );
    }

    #[tokio::test]
    async fn concurrent_refreshes_for_one_cache_path_share_one_operation() {
        let coordinator = ModelsDevRefreshCoordinator::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let first_calls = Arc::clone(&calls);
        let path = PathBuf::from("pricing.json");
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);

        let first = coordinator.refresh(path.clone(), now, async move {
            first_calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            true
        });
        let second = coordinator.refresh(path, now, async {
            panic!("the second caller must await the first operation");
        });

        assert!(tokio::join!(first, second).0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_refresh_is_not_retried_within_the_attempt_window() {
        let coordinator = ModelsDevRefreshCoordinator::default();
        let path = PathBuf::from("pricing.json");
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);

        assert!(
            !coordinator
                .refresh(path.clone(), now, async { false })
                .await
        );
        assert!(
            !coordinator
                .refresh(path, now + Duration::from_secs(60), async {
                    panic!("the 15-minute bound must suppress this attempt");
                })
                .await
        );
    }

    #[test]
    fn cache_path_uses_the_existing_per_user_cache_root() {
        let cache_root = ModelsDevCache::default_cache_root().expect("per-user cache root");
        assert_eq!(
            ModelsDevCache::cache_path(None),
            cache_root.join("model-pricing").join("models-dev-v1.json")
        );
    }
}

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex as AsyncMutex, watch};

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const REFRESH_ATTEMPT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Per-token pricing decoded from the models.dev catalog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynamicModelPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_input_cost_per_token: Option<f64>,
    pub cache_write_input_cost_per_token: Option<f64>,
    pub threshold_tokens: Option<u64>,
    pub input_cost_per_token_above_threshold: Option<f64>,
    pub output_cost_per_token_above_threshold: Option<f64>,
    pub cache_read_input_cost_per_token_above_threshold: Option<f64>,
    pub cache_write_input_cost_per_token_above_threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct ModelsDevCatalog {
    providers: HashMap<String, ModelsDevProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ModelsDevCatalogWire {
    Envelope {
        providers: HashMap<String, ModelsDevProvider>,
    },
    ProviderMap(HashMap<String, ModelsDevProvider>),
}

impl<'de> Deserialize<'de> for ModelsDevCatalog {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let providers = match ModelsDevCatalogWire::deserialize(deserializer)? {
            ModelsDevCatalogWire::Envelope { providers }
            | ModelsDevCatalogWire::ProviderMap(providers) => providers,
        };
        Ok(Self {
            providers: providers
                .into_iter()
                .map(|(key, provider)| {
                    (
                        normalize_provider_id(provider.id.as_deref().unwrap_or(&key)),
                        provider,
                    )
                })
                .collect(),
        })
    }
}

impl ModelsDevCatalog {
    #[cfg(test)]
    fn decode(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    fn lookup(&self, provider_id: &str, model_id: &str) -> Option<DynamicModelPricing> {
        let provider = self.providers.get(&normalize_provider_id(provider_id))?;
        let candidates = model_id_candidates(model_id);
        for candidate in &candidates {
            if let Some(model) = provider.models.get(candidate)
                && let Some(pricing) = DynamicModelPricing::from_model(model)
            {
                return Some(pricing);
            }
        }
        provider.models.values().find_map(|model| {
            let model_candidates = model_id_candidates(&model.id);
            candidates
                .iter()
                .any(|candidate| model_candidates.contains(candidate))
                .then(|| DynamicModelPricing::from_model(model))
                .flatten()
        })
    }

    fn is_plausible_refresh(&self) -> bool {
        ["openai", "anthropic"].into_iter().all(|provider_id| {
            self.providers
                .get(provider_id)
                .is_some_and(|provider| provider.models.values().any(ModelsDevModel::is_priceable))
        })
    }

    fn merge_priceable_entries_from(&mut self, cached: &Self) {
        for (provider_id, cached_provider) in &cached.providers {
            let provider = self
                .providers
                .entry(provider_id.clone())
                .or_insert_with(|| cached_provider.clone());
            let present_ids: HashSet<String> = provider
                .models
                .values()
                .filter(|model| model.is_priceable())
                .map(|model| stable_model_identity(&model.id))
                .collect();
            for (model_key, cached_model) in &cached_provider.models {
                if !cached_model.is_priceable()
                    || present_ids.contains(&stable_model_identity(&cached_model.id))
                {
                    continue;
                }
                let mut fallback_key = model_key.clone();
                if provider.models.contains_key(&fallback_key) {
                    fallback_key = format!(
                        "codexbar-fallback:{model_key}:{}",
                        normalize_model_id(&cached_model.id)
                    );
                }
                provider.models.insert(fallback_key, cached_model.clone());
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevProvider {
    id: Option<String>,
    #[serde(default)]
    models: HashMap<String, ModelsDevModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevModel {
    id: String,
    cost: Option<ModelsDevCost>,
}

impl ModelsDevModel {
    fn is_priceable(&self) -> bool {
        self.cost.as_ref().is_some_and(|cost| {
            cost.input.is_some_and(|rate| valid_number(&rate))
                && cost.output.is_some_and(|rate| valid_number(&rate))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevCost {
    input: Option<f64>,
    output: Option<f64>,
    #[serde(rename = "cache_read")]
    cache_read: Option<f64>,
    #[serde(rename = "cache_write")]
    cache_write: Option<f64>,
    #[serde(rename = "context_over_200k")]
    context_over_200k: Option<ModelsDevContextCost>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevContextCost {
    input: Option<f64>,
    output: Option<f64>,
    #[serde(rename = "cache_read")]
    cache_read: Option<f64>,
    #[serde(rename = "cache_write")]
    cache_write: Option<f64>,
}

impl DynamicModelPricing {
    fn from_model(model: &ModelsDevModel) -> Option<Self> {
        let cost = model.cost.as_ref()?;
        let input = cost.input.filter(valid_number)?;
        let output = cost.output.filter(valid_number)?;
        let above = cost.context_over_200k.as_ref();
        Some(Self {
            input_cost_per_token: per_token(input),
            output_cost_per_token: per_token(output),
            cache_read_input_cost_per_token: cost.cache_read.filter(valid_number).map(per_token),
            cache_write_input_cost_per_token: cost.cache_write.filter(valid_number).map(per_token),
            threshold_tokens: above.is_some().then_some(200_000),
            input_cost_per_token_above_threshold: above
                .and_then(|cost| cost.input)
                .filter(valid_number)
                .map(per_token),
            output_cost_per_token_above_threshold: above
                .and_then(|cost| cost.output)
                .filter(valid_number)
                .map(per_token),
            cache_read_input_cost_per_token_above_threshold: above
                .and_then(|cost| cost.cache_read)
                .filter(valid_number)
                .map(per_token),
            cache_write_input_cost_per_token_above_threshold: above
                .and_then(|cost| cost.cache_write)
                .filter(valid_number)
                .map(per_token),
        })
    }
}

fn valid_number(rate: &f64) -> bool {
    rate.is_finite() && *rate >= 0.0
}

fn per_token(rate: f64) -> f64 {
    rate / 1_000_000.0
}

fn normalize_provider_id(provider_id: &str) -> String {
    provider_id.trim().to_ascii_lowercase()
}

fn normalize_model_id(model_id: &str) -> String {
    model_id.trim().to_string()
}

fn stable_model_identity(model_id: &str) -> String {
    let model_id = normalize_model_id(model_id);
    if let Some((base, suffix)) = model_id.split_once('@') {
        if suffix == "default" {
            return base.to_string();
        }
        if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            return format!("{base}-{suffix}");
        }
    }
    model_id
}

fn model_id_candidates(model_id: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    append_model_candidate(&mut candidates, model_id.to_string());
    let mut index = 0;
    while index < candidates.len() {
        let candidate = candidates[index].clone();
        if let Some(rest) = candidate.strip_prefix("openai/") {
            append_model_candidate(&mut candidates, rest.to_string());
        }
        if let Some(rest) = candidate.strip_prefix("anthropic.") {
            append_model_candidate(&mut candidates, rest.to_string());
        }
        if candidate.contains("claude-")
            && let Some((_, tail)) = candidate.rsplit_once('.')
            && tail.starts_with("claude-")
        {
            append_model_candidate(&mut candidates, tail.to_string());
        }
        if let Some((base, suffix)) = candidate.split_once('@') {
            if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                append_model_candidate(&mut candidates, format!("{base}-{suffix}"));
            }
            append_model_candidate(&mut candidates, base.to_string());
        } else if candidate.starts_with("claude-") {
            append_model_candidate(&mut candidates, format!("{candidate}@default"));
        }
        if let Some(base) = candidate.strip_suffix("-v1:0") {
            append_model_candidate(&mut candidates, base.to_string());
        }
        if let Some(base) = strip_date_suffix(&candidate) {
            append_model_candidate(&mut candidates, base.to_string());
        }
        index += 1;
    }
    candidates
}

fn append_model_candidate(candidates: &mut Vec<String>, candidate: String) {
    let candidate = normalize_model_id(&candidate);
    if !candidate.is_empty() && !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn strip_date_suffix(model_id: &str) -> Option<&str> {
    let suffix = model_id.rsplit_once('-')?.1;
    if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(&model_id[..model_id.len() - suffix.len() - 1]);
    }
    if suffix.len() != 2 || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let without_day = &model_id[..model_id.len() - 3];
    let month = without_day.rsplit_once('-')?.1;
    if month.len() != 2 || !month.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let without_month = &without_day[..without_day.len() - 3];
    let year = without_month.rsplit_once('-')?.1;
    if year.len() == 4 && year.bytes().all(|byte| byte.is_ascii_digit()) {
        Some(&without_month[..without_month.len() - 5])
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevCacheArtifact {
    version: u32,
    fetched_at_unix_ms: u64,
    catalog: ModelsDevCatalog,
}

impl ModelsDevCacheArtifact {
    fn new(catalog: ModelsDevCatalog, fetched_at: SystemTime) -> Self {
        Self {
            version: ModelsDevCache::ARTIFACT_VERSION,
            fetched_at_unix_ms: unix_ms(fetched_at),
            catalog,
        }
    }

    fn fetched_at(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.fetched_at_unix_ms)
    }

    fn is_stale(&self, now: SystemTime) -> bool {
        now.duration_since(self.fetched_at()).unwrap_or_default() > CACHE_TTL
    }
}

struct ModelsDevCacheLoad {
    artifact: Option<Arc<ModelsDevCacheArtifact>>,
    is_stale: bool,
}

struct ModelsDevCacheMemoEntry {
    modified_at: Option<SystemTime>,
    size: Option<u64>,
    artifact: Option<Arc<ModelsDevCacheArtifact>>,
}

static CACHE_MEMO: LazyLock<Mutex<HashMap<PathBuf, ModelsDevCacheMemoEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Stable hash of the prices that are actually in force.
///
/// The on-disk artifact is rewritten on a daily cadence even when no rate
/// moved: `fetched_at_unix_ms` changes, and `HashMap` provider keys shuffle.
/// Hashing those bytes would throw away every derived cache for nothing.
/// Decode first and fold only `(provider, model, cost fields)` in sorted order.
///
/// A stale catalog hashes as *no* catalog, because `lookup` refuses it: numbers
/// derived while it was stale were priced off the built-in rate card, and the
/// refresh that makes the same rates usable again has to invalidate them.
/// `ModelsDevCache::load` memoizes the decode by file identity, so the probes
/// that run on every index read and every cache hit do not re-parse the JSON.
pub fn pricing_content_fingerprint() -> u64 {
    fingerprint_catalog_at(SystemTime::now(), None)
}

fn fingerprint_catalog_at(now: SystemTime, cache_root: Option<&Path>) -> u64 {
    let load = ModelsDevCache::load(now, cache_root);
    if load.is_stale {
        return 0;
    }
    load.artifact
        .map(|artifact| fingerprint_catalog_prices(&artifact.catalog))
        .unwrap_or(0)
}

fn fingerprint_catalog_prices(catalog: &ModelsDevCatalog) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut providers: Vec<_> = catalog.providers.iter().collect();
    providers.sort_by(|left, right| left.0.cmp(right.0));
    for (provider_id, provider) in providers {
        mix_framed(&mut hash, provider_id.as_bytes());
        let mut models: Vec<_> = provider.models.iter().collect();
        models.sort_by(|left, right| left.0.cmp(right.0).then(left.1.id.cmp(&right.1.id)));
        for (model_key, model) in models {
            mix_framed(&mut hash, model_key.as_bytes());
            mix_framed(&mut hash, model.id.as_bytes());
            mix_cost_fields(&mut hash, model.cost.as_ref());
        }
    }
    hash
}

fn mix_cost_fields(hash: &mut u64, cost: Option<&ModelsDevCost>) {
    let Some(cost) = cost else {
        mix_fingerprint(hash, &[0]);
        return;
    };
    mix_fingerprint(hash, &[1]);
    mix_opt_f64(hash, cost.input);
    mix_opt_f64(hash, cost.output);
    mix_opt_f64(hash, cost.cache_read);
    mix_opt_f64(hash, cost.cache_write);
    match &cost.context_over_200k {
        Some(above) => {
            mix_fingerprint(hash, &[1]);
            mix_opt_f64(hash, above.input);
            mix_opt_f64(hash, above.output);
            mix_opt_f64(hash, above.cache_read);
            mix_opt_f64(hash, above.cache_write);
        }
        None => mix_fingerprint(hash, &[0]),
    }
}

fn mix_opt_f64(hash: &mut u64, value: Option<f64>) {
    match value {
        Some(number) if number.is_finite() => {
            mix_fingerprint(hash, &[1]);
            mix_fingerprint(hash, &number.to_le_bytes());
        }
        Some(_) => mix_fingerprint(hash, &[2]),
        None => mix_fingerprint(hash, &[0]),
    }
}

/// Length-prefixed, so `("ab", "c")` cannot hash the same as `("a", "bc")`.
fn mix_framed(hash: &mut u64, bytes: &[u8]) {
    mix_fingerprint(hash, &(bytes.len() as u64).to_le_bytes());
    mix_fingerprint(hash, bytes);
}

fn mix_fingerprint(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

struct ModelsDevCache;

impl ModelsDevCache {
    const ARTIFACT_VERSION: u32 = 1;

    fn default_cache_root() -> Option<PathBuf> {
        dirs::cache_dir().map(|path| path.join("CodexBar"))
    }

    fn cache_path(cache_root: Option<&Path>) -> PathBuf {
        cache_root
            .map(Path::to_path_buf)
            .or_else(Self::default_cache_root)
            .map(|root| {
                root.join("model-pricing")
                    .join(format!("models-dev-v{}.json", Self::ARTIFACT_VERSION))
            })
            .unwrap_or_default()
    }
}

fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

impl ModelsDevCache {
    fn load(now: SystemTime, cache_root: Option<&Path>) -> ModelsDevCacheLoad {
        let cache_path = Self::cache_path(cache_root);
        if cache_path.as_os_str().is_empty() {
            return ModelsDevCacheLoad {
                artifact: None,
                is_stale: true,
            };
        }
        let cache_path = standardized_cache_path(&cache_path);
        let (modified_at, size) = file_identity(&cache_path);
        let memoized = {
            let memo = CACHE_MEMO.lock().expect("models.dev cache memo lock");
            memo.get(&cache_path)
                .filter(|entry| entry.modified_at == modified_at && entry.size == size)
                .map(|entry| entry.artifact.clone())
        };
        let artifact = memoized.unwrap_or_else(|| {
            let artifact = fs::read(&cache_path)
                .ok()
                .and_then(|contents| {
                    serde_json::from_slice::<ModelsDevCacheArtifact>(&contents).ok()
                })
                .filter(|artifact| artifact.version == Self::ARTIFACT_VERSION)
                .map(Arc::new);
            CACHE_MEMO
                .lock()
                .expect("models.dev cache memo lock")
                .insert(
                    cache_path,
                    ModelsDevCacheMemoEntry {
                        modified_at,
                        size,
                        artifact: artifact.clone(),
                    },
                );
            artifact
        });
        let is_stale = artifact
            .as_ref()
            .is_none_or(|artifact| artifact.is_stale(now));
        ModelsDevCacheLoad { artifact, is_stale }
    }
}

fn file_identity(path: &Path) -> (Option<SystemTime>, Option<u64>) {
    let Ok(metadata) = fs::metadata(path) else {
        return (None, None);
    };
    (metadata.modified().ok(), Some(metadata.len()))
}

fn standardized_cache_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|current_dir| current_dir.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}

impl ModelsDevCache {
    fn save(catalog: ModelsDevCatalog, fetched_at: SystemTime, cache_root: Option<&Path>) -> bool {
        let cache_path = Self::cache_path(cache_root);
        if cache_path.as_os_str().is_empty() {
            return false;
        }
        let cache_path = standardized_cache_path(&cache_path);
        let Some(parent) = cache_path.parent() else {
            return false;
        };
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
        let artifact = Arc::new(ModelsDevCacheArtifact::new(catalog, fetched_at));
        let Ok(contents) = serde_json::to_vec(&*artifact) else {
            return false;
        };
        // Write via a temp file and rename. `File::create` truncates the live
        // cache to zero before the new bytes land, so a second process reading
        // in that window (or this one dying mid-write) loses a good catalog and
        // silently reports no prices until a network refresh succeeds. The
        // in-process refresh coordinator does not span processes.
        if crate::secure_file::atomic_write(&cache_path, &contents).is_err() {
            return false;
        }
        let (modified_at, size) = file_identity(&cache_path);
        CACHE_MEMO
            .lock()
            .expect("models.dev cache memo lock")
            .insert(
                cache_path,
                ModelsDevCacheMemoEntry {
                    modified_at,
                    size,
                    artifact: Some(artifact),
                },
            );
        true
    }
}

#[derive(Default)]
struct ModelsDevRefreshCoordinator {
    state: Arc<AsyncMutex<ModelsDevRefreshState>>,
}

#[derive(Default)]
struct ModelsDevRefreshState {
    in_flight: HashMap<PathBuf, watch::Receiver<Option<bool>>>,
    last_attempt: HashMap<PathBuf, SystemTime>,
}

impl ModelsDevRefreshCoordinator {
    async fn refresh<F>(&self, cache_path: PathBuf, now: SystemTime, operation: F) -> bool
    where
        F: Future<Output = bool> + Send + 'static,
    {
        let cache_path = standardized_cache_path(&cache_path);
        let mut state = self.state.lock().await;
        if let Some(in_flight) = state.in_flight.get(&cache_path) {
            let receiver = in_flight.clone();
            drop(state);
            return wait_for_refresh(receiver).await;
        }
        if state
            .last_attempt
            .get(&cache_path)
            .is_some_and(|last_attempt| {
                now.duration_since(*last_attempt).unwrap_or_default() < REFRESH_ATTEMPT_WINDOW
            })
        {
            return false;
        }

        state.last_attempt.insert(cache_path.clone(), now);
        let (sender, receiver) = watch::channel(None);
        state.in_flight.insert(cache_path.clone(), receiver.clone());
        drop(state);

        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let result = operation.await;
            let _ = sender.send(Some(result));
            state
                .lock()
                .await
                .in_flight
                .retain(|path, _| path != &cache_path);
        });
        wait_for_refresh(receiver).await
    }
}

async fn wait_for_refresh(mut receiver: watch::Receiver<Option<bool>>) -> bool {
    loop {
        if let Some(result) = *receiver.borrow() {
            return result;
        }
        if receiver.changed().await.is_err() {
            return false;
        }
    }
}

static REFRESH_COORDINATOR: LazyLock<ModelsDevRefreshCoordinator> =
    LazyLock::new(ModelsDevRefreshCoordinator::default);

/// Looks up a cached models.dev price for a provider/model pair.
pub fn lookup(provider_id: &str, model_id: &str) -> Option<DynamicModelPricing> {
    let load = ModelsDevCache::load(SystemTime::now(), None);
    (!load.is_stale)
        .then_some(load.artifact)
        .flatten()
        .and_then(|artifact| artifact.catalog.lookup(provider_id, model_id))
}

/// Refreshes the models.dev cache once when supplied models lack cached pricing.
///
/// Returns true only if at least one supplied model has pricing after the coordinated refresh.
pub async fn refresh_unknown_models_if_needed(
    provider_id: &str,
    model_ids: &HashSet<String>,
) -> bool {
    if model_ids.is_empty() {
        return false;
    }
    refresh_unknown_models_at(provider_id, model_ids, SystemTime::now(), None).await
}

async fn refresh_unknown_models_at(
    provider_id: &str,
    model_ids: &HashSet<String>,
    now: SystemTime,
    cache_root: Option<&Path>,
) -> bool {
    let load = ModelsDevCache::load(now, cache_root);
    let unknown_models: Vec<String> = if load.is_stale {
        model_ids.iter().cloned().collect()
    } else {
        model_ids
            .iter()
            .filter(|model_id| {
                load.artifact
                    .as_ref()
                    .and_then(|artifact| artifact.catalog.lookup(provider_id, model_id))
                    .is_none()
            })
            .cloned()
            .collect()
    };
    if unknown_models.is_empty() {
        return true;
    }
    if load.artifact.as_ref().is_some_and(|artifact| {
        now.duration_since(artifact.fetched_at())
            .unwrap_or_default()
            < REFRESH_ATTEMPT_WINDOW
    }) {
        return false;
    }

    let cache_path = ModelsDevCache::cache_path(cache_root);
    if cache_path.as_os_str().is_empty() {
        return false;
    }
    let cache_root = cache_root.map(Path::to_path_buf);
    let refresh_cache_root = cache_root.clone();
    let _ = REFRESH_COORDINATOR
        .refresh(cache_path, now, async move {
            refresh_catalog(now, refresh_cache_root.as_deref()).await
        })
        .await;

    let refreshed = ModelsDevCache::load(now, cache_root.as_deref());
    !refreshed.is_stale
        && unknown_models.iter().any(|model_id| {
            refreshed
                .artifact
                .as_ref()
                .and_then(|artifact| artifact.catalog.lookup(provider_id, model_id))
                .is_some()
        })
}

async fn refresh_catalog(now: SystemTime, cache_root: Option<&Path>) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
    else {
        return false;
    };
    let Ok(response) = client.get(MODELS_DEV_URL).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(mut catalog) = response.json::<ModelsDevCatalog>().await else {
        return false;
    };
    if !catalog.is_plausible_refresh() {
        return false;
    }
    if let Some(cached) = ModelsDevCache::load(now, cache_root).artifact {
        catalog.merge_priceable_entries_from(&cached.catalog);
    }
    ModelsDevCache::save(catalog, now, cache_root)
}
