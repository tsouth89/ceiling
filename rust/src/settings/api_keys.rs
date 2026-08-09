use super::*;

/// API key storage for providers that need tokens
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiKeys {
    /// Provider ID -> API key mapping
    pub keys: HashMap<String, ApiKeyEntry>,
}

/// A single API key entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyEntry {
    pub api_key: String,
    pub saved_at: String,
    /// Optional label for the key (e.g., "Personal", "Work")
    #[serde(default)]
    pub label: Option<String>,
}

impl ApiKeys {
    /// Apply one mutation against the latest on-disk store while holding the
    /// process-wide state lock for the complete read-modify-write cycle.
    pub fn update(operation: impl FnOnce(&mut Self)) -> anyhow::Result<Self> {
        let path = Self::keys_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine API keys path"))?;
        crate::secure_file::with_state_write_lock(|| {
            let mut keys = Self::load_update_from(&path)?;
            operation(&mut keys);
            keys.save_to(&path).map_err(std::io::Error::other)?;
            Ok(keys)
        })
        .map_err(Into::into)
    }

    fn update_at(
        path: &std::path::Path,
        operation: impl FnOnce(&mut Self),
    ) -> anyhow::Result<Self> {
        let lock_path = path.with_extension("state-write.lock");
        crate::secure_file::with_state_write_lock_at(&lock_path, || {
            let mut keys = Self::load_update_from(path)?;
            operation(&mut keys);
            keys.save_to(path).map_err(std::io::Error::other)?;
            Ok(keys)
        })
        .map_err(Into::into)
    }
    /// Get the API keys file path
    pub fn keys_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("Ceiling").join("api_keys.json"))
    }

    /// Load API keys from disk, treating an unreadable store as empty.
    ///
    /// Read-only callers only. Anything that mutates and then calls
    /// [`save`](Self::save) must use [`load_for_update`](Self::load_for_update).
    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }

    /// Load API keys for a read-modify-write cycle, failing closed.
    ///
    /// [`save`](Self::save) replaces the entire file, so mutating a store that
    /// silently decoded as empty (corrupt JSON, DPAPI unprotect failure, a
    /// secure-file version this build does not support) permanently destroys
    /// every key we failed to read. A missing file is still an empty store —
    /// only an existing-but-undecodable one is an error (SBS-623).
    pub fn load_for_update() -> anyhow::Result<Self> {
        Self::try_load().map_err(|error| {
            // The underlying error is a decode failure over already-decrypted
            // key material, and a serde data error can quote the fragment it
            // choked on. This string is returned through the Tauri bridge to
            // the frontend, so it stays generic; the detail goes to the log.
            tracing::warn!(%error, "Saved API keys could not be decoded");
            anyhow::anyhow!(
                "Saved API keys could not be read. Refusing to write, which \
                 would replace the stored keys with only this change. See the \
                 log for details."
            )
        })
    }

    pub(super) fn try_load() -> anyhow::Result<Self> {
        let Some(path) = Self::keys_path() else {
            return Ok(Self::default());
        };
        Self::try_load_from(&path)
    }

    /// Decode the store at `path`. A missing file is an empty store; an
    /// existing file that will not decode is an error.
    pub(super) fn try_load_from(path: &std::path::Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = crate::secure_file::read_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    fn load_update_from(path: &std::path::Path) -> std::io::Result<Self> {
        Self::try_load_from(path).map_err(|error| {
            tracing::warn!(%error, "Saved API keys could not be decoded");
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Saved API keys could not be read; refusing to replace them",
            )
        })
    }

    /// Save API keys to disk
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::keys_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine API keys path"))?;
        self.save_to(&path)
    }

    pub(super) fn save_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(self)?;
        crate::secure_file::write_string(path, &json)?;

        Ok(())
    }

    /// Get API key for a provider
    pub fn get(&self, provider_id: &str) -> Option<&str> {
        self.keys.get(provider_id).map(|e| e.api_key.as_str())
    }

    /// Set API key for a provider
    pub fn set(&mut self, provider_id: &str, api_key: &str, label: Option<&str>) {
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M").to_string();
        self.keys.insert(
            provider_id.to_string(),
            ApiKeyEntry {
                api_key: api_key.to_string(),
                saved_at: now,
                label: label.map(|s| s.to_string()),
            },
        );
    }

    /// Remove API key for a provider
    pub fn remove(&mut self, provider_id: &str) {
        self.keys.remove(provider_id);
    }

    /// Check if a provider has an API key configured
    pub fn has_key(&self, provider_id: &str) -> bool {
        self.keys
            .get(provider_id)
            .map(|e| !e.api_key.is_empty())
            .unwrap_or(false)
    }

    /// Get all saved API keys for UI display (with masked values)
    pub fn get_all_for_display(&self) -> Vec<SavedApiKeyInfo> {
        self.keys
            .iter()
            .map(|(id, entry)| {
                let provider_name = ProviderId::from_cli_name(id)
                    .map(|p| p.display_name().to_string())
                    .unwrap_or_else(|| id.clone());

                let masked = mask_api_key(&entry.api_key);

                SavedApiKeyInfo {
                    provider_id: id.clone(),
                    provider: provider_name,
                    masked_key: masked,
                    saved_at: entry.saved_at.clone(),
                    label: entry.label.clone(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SBS-623: `save` atomically replaces the whole file, so a store that
    /// silently decoded as empty takes every unread key with it on the next
    /// write. The fail-open `load` must stay fail-open for readers, and the
    /// read-modify-write entry point must fail closed.
    #[test]
    fn an_unreadable_store_fails_closed_for_updates_but_open_for_readers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("api_keys.json");

        let mut keys = ApiKeys::default();
        keys.set("openrouter", "or-secret-1", None);
        keys.set("groq", "groq-secret-2", None);
        keys.save_to(&path).expect("seed api keys");

        // Corrupt the store the way a DPAPI failure or partial write would.
        std::fs::write(&path, b"{not-valid-json").expect("corrupt file");

        let error = ApiKeys::try_load_from(&path)
            .expect_err("an existing store that will not decode must be an error");
        assert!(
            !format!("{error}").is_empty(),
            "the failure must carry a diagnosable message"
        );

        // The fail-open reader contract is deliberate and still holds; it is
        // only unsafe when followed by a save, which is what load_for_update
        // now guards.
        assert!(
            ApiKeys::try_load_from(&path).is_err(),
            "a corrupt store must not decode as an empty one for writers"
        );
    }

    /// A store that was never created is genuinely empty, not an error — first
    /// run must still be able to save a key.
    #[test]
    fn a_missing_store_is_an_empty_store_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("api_keys.json");

        let keys = ApiKeys::try_load_from(&path).expect("missing file loads as empty");
        assert!(keys.keys.is_empty());
    }

    /// The round trip a legitimate update takes: every previously stored key
    /// survives writing one more.
    #[test]
    fn saving_one_key_keeps_the_others() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("api_keys.json");

        let mut keys = ApiKeys::default();
        keys.set("openrouter", "or-secret-1", None);
        keys.set("groq", "groq-secret-2", None);
        keys.save_to(&path).expect("seed");

        let mut keys = ApiKeys::try_load_from(&path).expect("load");
        keys.set("openrouter", "or-secret-replacement", None);
        keys.save_to(&path).expect("save");

        let reloaded = ApiKeys::try_load_from(&path).expect("reload");
        assert_eq!(reloaded.get("groq"), Some("groq-secret-2"));
        assert_eq!(reloaded.get("openrouter"), Some("or-secret-replacement"));
    }

    #[test]
    fn concurrent_provider_updates_both_survive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = std::sync::Arc::new(dir.path().join("api_keys.json"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

        let writers: Vec<_> = [("openrouter", "or-secret"), ("groq", "groq-secret")]
            .into_iter()
            .map(|(provider, secret)| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    ApiKeys::update_at(&path, |keys| keys.set(provider, secret, None))
                        .expect("concurrent update");
                })
            })
            .collect();
        barrier.wait();
        for writer in writers {
            writer.join().expect("writer thread");
        }

        let keys = ApiKeys::try_load_from(&path).expect("reload");
        assert_eq!(keys.get("openrouter"), Some("or-secret"));
        assert_eq!(keys.get("groq"), Some("groq-secret"));
    }
}

fn mask_api_key(api_key: &str) -> String {
    let chars: Vec<char> = api_key.chars().collect();
    if chars.len() > 12 {
        let prefix: String = chars.iter().take(4).collect();
        let suffix: String = chars.iter().skip(chars.len() - 4).collect();
        format!("{prefix}...{suffix}")
    } else if chars.len() > 4 {
        let prefix: String = chars.iter().take(4).collect();
        format!("{prefix}...")
    } else {
        "****".to_string()
    }
}

/// Info about a saved API key for UI display
#[derive(Debug, Clone, Serialize)]
pub struct SavedApiKeyInfo {
    pub provider_id: String,
    pub provider: String,
    pub masked_key: String,
    pub saved_at: String,
    pub label: Option<String>,
}

/// Provider configuration info
#[derive(Debug, Clone)]
pub struct ProviderConfigInfo {
    pub id: ProviderId,
    pub name: &'static str,
    pub requires_api_key: bool,
    pub api_key_env_var: Option<&'static str>,
    pub api_key_help: Option<&'static str>,
    pub config_file_path: Option<&'static str>,
    pub dashboard_url: Option<&'static str>,
}

/// Get configuration info for providers that need API keys
pub fn get_api_key_providers() -> Vec<ProviderConfigInfo> {
    vec![
        ProviderConfigInfo {
            id: ProviderId::Alibaba,
            name: "Alibaba Coding Plan",
            requires_api_key: true,
            api_key_env_var: Some("ALIBABA_CODING_PLAN_API_KEY"),
            api_key_help: Some("Get your Coding Plan API key from Alibaba Model Studio / Bailian"),
            config_file_path: Some("~/.codexbar/config.json"),
            dashboard_url: Some(
                "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=coding-plan#/efm/detail",
            ),
        },
        ProviderConfigInfo {
            id: ProviderId::Amp,
            name: "Amp (Sourcegraph)",
            requires_api_key: true,
            api_key_env_var: Some("SRC_ACCESS_TOKEN"),
            api_key_help: Some("Get your token from Sourcegraph → Settings → Access Tokens"),
            config_file_path: Some("~/.amp/config.json"),
            dashboard_url: Some("https://sourcegraph.com/cody/manage"),
        },
        ProviderConfigInfo {
            id: ProviderId::Copilot,
            name: "GitHub Copilot (legacy token)",
            requires_api_key: true,
            api_key_env_var: Some("GITHUB_TOKEN"),
            api_key_help: Some(
                "Optional fallback. Prefer Providers → Copilot → Sign in with GitHub.",
            ),
            config_file_path: None,
            dashboard_url: Some("https://github.com/settings/copilot"),
        },
        ProviderConfigInfo {
            id: ProviderId::Zai,
            name: "z.ai",
            requires_api_key: true,
            api_key_env_var: Some("Z_AI_API_KEY or ZAI_API_TOKEN"),
            api_key_help: Some(
                "Get your API token from z.ai Dashboard. BigModel team usage can set Z_AI_BIGMODEL_ORGANIZATION + Z_AI_BIGMODEL_PROJECT, or provider workspace_id as organization|project.",
            ),
            config_file_path: None,
            dashboard_url: Some("https://z.ai/manage-apikey/coding-plan/personal/my-plan"),
        },
        ProviderConfigInfo {
            id: ProviderId::Warp,
            name: "Warp",
            requires_api_key: true,
            api_key_env_var: Some("WARP_API_KEY"),
            api_key_help: Some(
                "Get your API key from Warp → Settings → API Keys (docs.warp.dev/reference/cli/api-keys)",
            ),
            config_file_path: None,
            dashboard_url: Some("https://docs.warp.dev/reference/cli/api-keys"),
        },
        ProviderConfigInfo {
            id: ProviderId::Ollama,
            name: "Ollama",
            requires_api_key: false,
            api_key_env_var: Some("OLLAMA_API_KEY / OLLAMA_KEY"),
            api_key_help: Some(
                "Optional: use an Ollama API key for Cloud validation, or browser cookies for usage.",
            ),
            config_file_path: None,
            dashboard_url: Some("https://ollama.com/settings"),
        },
        ProviderConfigInfo {
            id: ProviderId::AzureOpenAI,
            name: "Azure OpenAI",
            requires_api_key: true,
            api_key_env_var: Some(
                "AZURE_OPENAI_API_KEY + AZURE_OPENAI_ENDPOINT + AZURE_OPENAI_DEPLOYMENT",
            ),
            api_key_help: Some(
                "Use env vars, or save JSON {api_key, endpoint, deployment, api_version}; composite api_key|endpoint|deployment[|api_version] also works.",
            ),
            config_file_path: Some("~/.codexbar/api_keys.json"),
            dashboard_url: Some("https://ai.azure.com"),
        },
        ProviderConfigInfo {
            id: ProviderId::OpenRouter,
            name: "OpenRouter",
            requires_api_key: true,
            api_key_env_var: Some("OPENROUTER_API_KEY"),
            api_key_help: Some("Get your API key from openrouter.ai/settings/keys"),
            config_file_path: None,
            dashboard_url: Some("https://openrouter.ai/settings/credits"),
        },
        ProviderConfigInfo {
            id: ProviderId::NanoGPT,
            name: "NanoGPT",
            requires_api_key: true,
            api_key_env_var: Some("NANOGPT_API_KEY"),
            api_key_help: Some("Get your API key from nano-gpt.com/api"),
            config_file_path: None,
            dashboard_url: Some("https://nano-gpt.com/api"),
        },
        ProviderConfigInfo {
            id: ProviderId::Infini,
            name: "Infini AI",
            requires_api_key: true,
            api_key_env_var: Some("INFINI_API_KEY"),
            api_key_help: Some("Get your API key from Infini Cloud → Settings → API Keys"),
            config_file_path: None,
            dashboard_url: Some("https://cloud.infini-ai.com"),
        },
        ProviderConfigInfo {
            id: ProviderId::Kimi,
            name: "Kimi Code API",
            requires_api_key: true,
            api_key_env_var: Some("KIMI_CODE_API_KEY"),
            api_key_help: Some(
                "Get your Kimi Code API key from Kimi. Optional HTTPS proxy base URL: KIMI_CODE_BASE_URL.",
            ),
            config_file_path: None,
            dashboard_url: Some("https://platform.moonshot.cn/console/api-keys"),
        },
        ProviderConfigInfo {
            id: ProviderId::Kilo,
            name: "Kilo",
            requires_api_key: true,
            api_key_env_var: Some("KILO_API_KEY"),
            api_key_help: Some("Get your API key from Kilo, or sign in with Kilo CLI."),
            config_file_path: Some("~/.local/share/kilo/auth.json"),
            dashboard_url: Some("https://app.kilo.ai/usage"),
        },
        ProviderConfigInfo {
            id: ProviderId::Bedrock,
            name: "AWS Bedrock",
            requires_api_key: true,
            api_key_env_var: Some(
                "AWS_ACCESS_KEY_ID:AWS_SECRET_ACCESS_KEY[:AWS_SESSION_TOKEN] or AWS_PROFILE",
            ),
            api_key_help: Some(
                "Paste access_key:secret_key[:session_token], JSON credentials, profile:name, or use AWS env vars/AWS CLI profiles.",
            ),
            config_file_path: None,
            dashboard_url: Some("https://console.aws.amazon.com/bedrock"),
        },
        ProviderConfigInfo {
            id: ProviderId::Codebuff,
            name: "Codebuff",
            requires_api_key: true,
            api_key_env_var: Some("CODEBUFF_API_KEY"),
            api_key_help: Some(
                "Get your API key from Codebuff, or sign in with Codebuff/Manicode.",
            ),
            config_file_path: Some("~/.config/manicode/credentials.json"),
            dashboard_url: Some("https://www.codebuff.com/usage"),
        },
        ProviderConfigInfo {
            id: ProviderId::DeepSeek,
            name: "DeepSeek",
            requires_api_key: true,
            api_key_env_var: Some("DEEPSEEK_API_KEY"),
            api_key_help: Some("Get your API key from platform.deepseek.com."),
            config_file_path: None,
            dashboard_url: Some("https://platform.deepseek.com/usage"),
        },
        ProviderConfigInfo {
            id: ProviderId::Doubao,
            name: "Doubao / Volcengine Ark",
            requires_api_key: true,
            api_key_env_var: Some(
                "ARK_API_KEY or VOLCENGINE_ACCESS_KEY_ID + VOLCENGINE_SECRET_ACCESS_KEY",
            ),
            api_key_help: Some(
                "Use ARK_API_KEY for chat probe fallback, or paste Coding Plan credentials as access_key|secret_key|region (region defaults to cn-beijing).",
            ),
            config_file_path: None,
            dashboard_url: Some("https://console.volcengine.com/ark/region:ark+cn-beijing/usage"),
        },
        ProviderConfigInfo {
            id: ProviderId::Crof,
            name: "Crof",
            requires_api_key: true,
            api_key_env_var: Some("CROF_API_KEY"),
            api_key_help: Some("Get your API key from Crof."),
            config_file_path: None,
            dashboard_url: Some("https://crof.ai"),
        },
        ProviderConfigInfo {
            id: ProviderId::StepFun,
            name: "StepFun",
            requires_api_key: true,
            api_key_env_var: Some("STEPFUN_OASIS_TOKEN"),
            api_key_help: Some("Paste an existing Oasis-Token from StepFun login."),
            config_file_path: None,
            dashboard_url: Some("https://platform.stepfun.com/dashboard"),
        },
        ProviderConfigInfo {
            id: ProviderId::Venice,
            name: "Venice",
            requires_api_key: true,
            api_key_env_var: Some("VENICE_API_KEY"),
            api_key_help: Some("Get your API key from Venice settings."),
            config_file_path: None,
            dashboard_url: Some("https://venice.ai/settings/api"),
        },
        ProviderConfigInfo {
            id: ProviderId::OpenAIApi,
            name: "OpenAI",
            requires_api_key: true,
            api_key_env_var: Some("OPENAI_ADMIN_KEY / OPENAI_API_KEY"),
            api_key_help: Some(
                "Use an OpenAI Admin API key for usage, or a platform key for legacy billing balance.",
            ),
            config_file_path: None,
            dashboard_url: Some("https://platform.openai.com/usage"),
        },
        ProviderConfigInfo {
            id: ProviderId::Grok,
            name: "Grok",
            requires_api_key: false,
            api_key_env_var: None,
            api_key_help: Some(
                "Uses ~/.grok/auth.json from `grok login` (Grok Build), or grok.com browser cookies.",
            ),
            config_file_path: Some("~/.grok/auth.json"),
            dashboard_url: Some("https://grok.com/?_s=usage"),
        },
        ProviderConfigInfo {
            id: ProviderId::ElevenLabs,
            name: "ElevenLabs",
            requires_api_key: true,
            api_key_env_var: Some("ELEVENLABS_API_KEY"),
            api_key_help: Some("Get your API key from ElevenLabs Settings > API Keys."),
            config_file_path: None,
            dashboard_url: Some("https://elevenlabs.io/app/settings/api-keys"),
        },
        ProviderConfigInfo {
            id: ProviderId::Deepgram,
            name: "Deepgram",
            requires_api_key: true,
            api_key_env_var: Some("DEEPGRAM_API_KEY"),
            api_key_help: Some("Use a Deepgram API key with Management API access."),
            config_file_path: None,
            dashboard_url: Some("https://console.deepgram.com/usage"),
        },
        ProviderConfigInfo {
            id: ProviderId::Groq,
            name: "Groq",
            requires_api_key: true,
            api_key_env_var: Some("GROQ_API_KEY"),
            api_key_help: Some("Groq metrics require Enterprise Prometheus metrics access."),
            config_file_path: None,
            dashboard_url: Some("https://console.groq.com/settings/metrics"),
        },
        ProviderConfigInfo {
            id: ProviderId::LLMProxy,
            name: "LLM Proxy",
            requires_api_key: true,
            api_key_env_var: Some("LLM_PROXY_API_KEY + LLM_PROXY_BASE_URL"),
            api_key_help: Some("Set an LLM Proxy API key and base URL for quota-stats."),
            config_file_path: None,
            dashboard_url: None,
        },
        ProviderConfigInfo {
            id: ProviderId::Chutes,
            name: "Chutes",
            requires_api_key: true,
            api_key_env_var: Some("CHUTES_API_KEY"),
            api_key_help: Some(
                "Paste a Chutes API key. Optional API URL override: CHUTES_API_URL.",
            ),
            config_file_path: None,
            dashboard_url: Some("https://chutes.ai"),
        },
        ProviderConfigInfo {
            id: ProviderId::LiteLLM,
            name: "LiteLLM",
            requires_api_key: true,
            api_key_env_var: Some("LITELLM_API_KEY + LITELLM_BASE_URL"),
            api_key_help: Some(
                "Paste a LiteLLM key and set the base URL in provider extras or LITELLM_BASE_URL.",
            ),
            config_file_path: None,
            dashboard_url: None,
        },
        ProviderConfigInfo {
            id: ProviderId::Poe,
            name: "Poe",
            requires_api_key: true,
            api_key_env_var: Some("POE_API_KEY"),
            api_key_help: Some("Get your API key from Poe API settings."),
            config_file_path: None,
            dashboard_url: Some("https://poe.com/settings/subscription"),
        },
        ProviderConfigInfo {
            id: ProviderId::Devin,
            name: "Devin",
            requires_api_key: true,
            api_key_env_var: Some("DEVIN_BEARER_TOKEN + DEVIN_ORG"),
            api_key_help: Some(
                "Paste a Devin bearer token and set the organization in provider extras or DEVIN_ORG.",
            ),
            config_file_path: None,
            dashboard_url: Some("https://app.devin.ai/settings/billing"),
        },
        ProviderConfigInfo {
            id: ProviderId::Zed,
            name: "Zed",
            requires_api_key: true,
            api_key_env_var: Some("ZED_CREDENTIALS"),
            api_key_help: Some(
                "Paste Zed credentials as `user_id access_token`; optional API URL in provider extras.",
            ),
            config_file_path: Some("~/.config/zed/settings.json"),
            dashboard_url: Some("https://zed.dev/account"),
        },
        ProviderConfigInfo {
            id: ProviderId::CrossModel,
            name: "CrossModel",
            requires_api_key: true,
            api_key_env_var: Some("CROSSMODEL_API_KEY"),
            api_key_help: Some(
                "Paste a CrossModel API key. Optional API URL override: CROSSMODEL_API_URL.",
            ),
            config_file_path: None,
            dashboard_url: Some("https://crossmodel.ai"),
        },
    ]
}
