use super::*;

// ── Provider summaries + ordering ─────────────────────────────────────

/// Lightweight provider entry returned to the UI after a reorder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSummary {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    pub order: u32,
}

/// Build `ProviderSummary` list honouring the persisted `provider_order`.
pub(crate) fn build_provider_summaries(settings: &Settings) -> Vec<ProviderSummary> {
    let order = settings.provider_display_order_names();

    let by_id: std::collections::HashMap<String, &ProviderId> = ProviderId::all()
        .iter()
        .map(|p| (p.cli_name().to_string(), p))
        .collect();

    order
        .iter()
        .enumerate()
        .filter_map(|(idx, id)| {
            by_id.get(id).map(|p| ProviderSummary {
                id: id.clone(),
                display_name: p.display_name().to_string(),
                enabled: settings.enabled_providers.contains(id),
                order: idx as u32,
            })
        })
        .collect()
}

#[tauri::command]
pub fn reorder_providers(
    app: tauri::AppHandle,
    ids: Vec<String>,
) -> Result<Vec<ProviderSummary>, String> {
    let settings = Settings::update(|settings| {
        settings.provider_order = codexbar::settings::normalize_provider_order(&ids)
    })
    .map_err(|e| e.to_string())?;
    crate::tray_bridge::refresh_tray_presentation(&app);
    // Notify open surfaces (tray flyout, pop-out window) so their provider grid
    // and cards re-render in the new order immediately after a drag-reorder.
    crate::events::emit_settings_changed(&app);
    Ok(build_provider_summaries(&settings))
}

// ── Per-provider cookie source + region ───────────────────────────────

/// Map a CLI-name string to a `ProviderId` whose cookie source is exposed in
/// the UI. Returns `None` for providers without a user-facing cookie source.
fn cookie_source_provider(provider_id: &str) -> Option<codexbar::core::ProviderId> {
    use codexbar::core::ProviderId;
    Some(match provider_id {
        "codex" => ProviderId::Codex,
        "claude" => ProviderId::Claude,
        "cursor" => ProviderId::Cursor,
        "opencode" => ProviderId::OpenCode,
        "factory" => ProviderId::Factory,
        "alibaba" => ProviderId::Alibaba,
        "kimi" | "kimik2" => ProviderId::Kimi,
        "minimax" => ProviderId::MiniMax,
        "augment" => ProviderId::Augment,
        "amp" => ProviderId::Amp,
        "ollama" => ProviderId::Ollama,
        "mistral" => ProviderId::Mistral,
        "qoder" => ProviderId::Qoder,
        "sakana" => ProviderId::Sakana,
        _ => return None,
    })
}

pub(crate) fn provider_cookie_source_lookup(
    settings: &Settings,
    provider_id: &str,
) -> Option<String> {
    cookie_source_provider(provider_id).map(|id| settings.cookie_source(id).to_string())
}

fn region_provider(provider_id: &str) -> Option<codexbar::core::ProviderId> {
    use codexbar::core::ProviderId;
    Some(match provider_id {
        "alibaba" => ProviderId::Alibaba,
        "zai" => ProviderId::Zai,
        "minimax" => ProviderId::MiniMax,
        _ => return None,
    })
}

pub(crate) fn provider_region_lookup(settings: &Settings, provider_id: &str) -> Option<String> {
    region_provider(provider_id).map(|id| {
        if id == codexbar::core::ProviderId::MiniMax {
            codexbar::providers::MiniMaxProvider::region_from_settings(Some(
                settings.api_region(id),
            ))
            .settings_value()
            .to_string()
        } else {
            settings.api_region(id).to_string()
        }
    })
}

pub(crate) fn provider_region_set(
    settings: &mut Settings,
    provider_id: &str,
    region: String,
) -> Result<(), String> {
    let id = region_provider(provider_id)
        .ok_or_else(|| format!("Provider '{provider_id}' does not have a region picker"))?;
    settings.set_api_region(id, region);
    Ok(())
}

#[tauri::command]
pub fn set_provider_region(provider_id: String, region: String) -> Result<(), String> {
    let region = region.trim();
    if region.is_empty()
        || !region_options_for(&provider_id)
            .iter()
            .any(|option| option.value == region)
    {
        return Err(format!(
            "Invalid region '{region}' for provider '{provider_id}'"
        ));
    }
    Settings::try_update(|settings| provider_region_set(settings, &provider_id, region.to_string()))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_provider_region(provider_id: String) -> Result<Option<String>, String> {
    Ok(provider_region_lookup(&Settings::load(), &provider_id))
}

fn workspace_provider(provider_id: &str) -> Option<codexbar::core::ProviderId> {
    use codexbar::core::ProviderId;
    Some(match provider_id {
        "openaiapi" => ProviderId::OpenAIApi,
        "litellm" => ProviderId::LiteLLM,
        "devin" => ProviderId::Devin,
        "opencodego" => ProviderId::OpenCodeGo,
        "zed" => ProviderId::Zed,
        _ => return None,
    })
}

#[tauri::command]
pub fn set_provider_workspace_id(provider_id: String, workspace_id: String) -> Result<(), String> {
    let id = workspace_provider(&provider_id).ok_or_else(|| {
        format!("Provider '{provider_id}' does not expose a workspace/project id")
    })?;
    let workspace_id = codexbar::settings::validate_provider_workspace_value(id, &workspace_id)?;
    Settings::try_update(|settings| {
        prevent_litellm_key_retargeting(id, settings.workspace_id(id), &workspace_id)?;
        settings.set_workspace_id(id, workspace_id);
        Ok(())
    })
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn prevent_litellm_key_retargeting(
    id: codexbar::core::ProviderId,
    current_workspace_id: &str,
    next_workspace_id: &str,
) -> Result<(), String> {
    litellm_workspace_change_allowed(
        id,
        current_workspace_id,
        next_workspace_id,
        ApiKeys::load().has_key(id.cli_name()),
    )
}

fn litellm_workspace_change_allowed(
    id: codexbar::core::ProviderId,
    current_workspace_id: &str,
    next_workspace_id: &str,
    has_saved_api_key: bool,
) -> Result<(), String> {
    if id != codexbar::core::ProviderId::LiteLLM || next_workspace_id.trim().is_empty() {
        return Ok(());
    }

    let current = current_workspace_id.trim().trim_end_matches('/');
    let next = next_workspace_id.trim().trim_end_matches('/');
    if current.eq_ignore_ascii_case(next) || !has_saved_api_key {
        return Ok(());
    }

    Err(
        "Remove the saved LiteLLM API key before changing the LiteLLM base URL, then save the key again for the new endpoint."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use codexbar::core::ProviderId;

    use super::{litellm_workspace_change_allowed, workspace_provider};

    #[test]
    fn maps_opencode_go_workspace_provider() {
        assert_eq!(
            workspace_provider("opencodego"),
            Some(ProviderId::OpenCodeGo)
        );
    }

    #[test]
    fn litellm_endpoint_change_requires_reentering_saved_key() {
        assert!(
            litellm_workspace_change_allowed(
                ProviderId::LiteLLM,
                "https://old.example.com",
                "https://new.example.com",
                true,
            )
            .is_err()
        );
        assert!(
            litellm_workspace_change_allowed(
                ProviderId::LiteLLM,
                "https://old.example.com",
                "https://old.example.com/",
                true,
            )
            .is_ok()
        );
        assert!(
            litellm_workspace_change_allowed(
                ProviderId::LiteLLM,
                "https://old.example.com",
                "",
                true,
            )
            .is_ok()
        );
        assert!(
            litellm_workspace_change_allowed(
                ProviderId::LiteLLM,
                "https://old.example.com",
                "https://new.example.com",
                false,
            )
            .is_ok()
        );
    }
}

#[tauri::command]
pub fn get_provider_workspace_id(provider_id: String) -> Result<Option<String>, String> {
    let Some(id) = workspace_provider(&provider_id) else {
        return Ok(None);
    };
    let value = Settings::load().workspace_id(id).trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

fn gateway_provider(provider_id: &str) -> Option<codexbar::core::ProviderId> {
    (provider_id == "wayfinder").then_some(codexbar::core::ProviderId::Wayfinder)
}

#[tauri::command]
pub fn get_provider_gateway_url(provider_id: String) -> Result<Option<String>, String> {
    let Some(id) = gateway_provider(&provider_id) else {
        return Ok(None);
    };
    Ok(Some(Settings::load().gateway_url(id).to_string()))
}

#[tauri::command]
pub fn set_provider_gateway_url(provider_id: String, gateway_url: String) -> Result<(), String> {
    let id = gateway_provider(&provider_id)
        .ok_or_else(|| format!("Provider '{provider_id}' does not expose a gateway URL"))?;
    let gateway_url = gateway_url.trim();
    codexbar::providers::wayfinder::parse_gateway_url(gateway_url)
        .map_err(|error| error.to_string())?;

    Settings::update(|settings| settings.set_gateway_url(id, gateway_url.to_string()))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

// ── Region option catalog ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegionOption {
    pub value: String,
    pub label: String,
}

/// Returns the API region options for a given provider.
/// Empty vec means the provider has no region picker.
pub fn region_options_for(provider_id: &str) -> Vec<RegionOption> {
    match provider_id {
        "alibaba" => codexbar::providers::AlibabaRegion::ALL
            .iter()
            .map(|region| RegionOption {
                value: region.settings_value().to_string(),
                label: region.display_name().to_string(),
            })
            .collect(),
        "zai" => vec![
            RegionOption {
                value: "global".to_string(),
                label: "Global".to_string(),
            },
            RegionOption {
                value: "china".to_string(),
                label: "China Mainland (BigModel)".to_string(),
            },
        ],
        "minimax" => vec![
            RegionOption {
                value: "global".to_string(),
                label: codexbar::providers::MiniMaxRegion::Global
                    .display_name()
                    .to_string(),
            },
            RegionOption {
                value: "cn".to_string(),
                label: codexbar::providers::MiniMaxRegion::ChinaMainland
                    .display_name()
                    .to_string(),
            },
        ],
        _ => Vec::new(),
    }
}

#[tauri::command]
pub fn get_provider_region_options(provider_id: String) -> Result<Vec<RegionOption>, String> {
    Ok(region_options_for(&provider_id))
}
