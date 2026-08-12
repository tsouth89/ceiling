import { useCallback, useEffect, useState } from "react";
import {
  getApiKeyProviders,
  getApiKeys,
  removeApiKey,
  setApiKey,
} from "../../../lib/tauri";
import type {
  ApiKeyInfoBridge,
  ApiKeyProviderInfoBridge,
} from "../../../types/bridge";

interface Props {
  providerId: string;
  onCredentialsChanged?: () => void;
}

/**
 * Per-provider API key management, embedded inside the ProviderDetailPane.
 * Mirrors the upstream macOS layout where credential management lives next
 * to provider state instead of in a separate tab.
 */
export function ApiKeySection({ providerId, onCredentialsChanged }: Props) {
  const [info, setInfo] = useState<ApiKeyProviderInfoBridge | null>(null);
  const [saved, setSaved] = useState<ApiKeyInfoBridge | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [editing, setEditing] = useState(false);
  const [editValue, setEditValue] = useState("");
  const [editLabel, setEditLabel] = useState("");

  const reload = useCallback(async (signal: { stale: boolean }) => {
    try {
      const [providers, keys] = await Promise.all([
        getApiKeyProviders(),
        getApiKeys(),
      ]);
      if (signal.stale) return;
      setInfo(providers.find((p) => p.id === providerId) ?? null);
      setSaved(keys.find((k) => k.providerId === providerId) ?? null);
    } catch (err: unknown) {
      if (signal.stale) return;
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (!signal.stale) setLoaded(true);
    }
  }, [providerId]);

  useEffect(() => {
    const signal = { stale: false };
    setLoaded(false);
    setEditing(false);
    setEditValue("");
    setEditLabel("");
    setError(null);
    setInfo(null);
    setSaved(null);
    void reload(signal);
    return () => { signal.stale = true; };
  }, [reload]);

  if (!loaded) return null;
  // Provider doesn't support API keys — render nothing.
  // Distinguish from "failed to load" by checking error state.
  if (!info && !error) return null;
  if (!info && error) {
    return (
      <section className="provider-detail-section">
        <h4>API Key</h4>
        <div className="settings-status settings-status--error">{error}</div>
      </section>
    );
  }
  // After the guards above, info is guaranteed non-null.
  if (!info) return null;

  const handleSave = async () => {
    if (!editValue.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const next = await setApiKey(
        providerId,
        editValue.trim(),
        editLabel.trim() || undefined,
      );
      setSaved(next.find((k) => k.providerId === providerId) ?? null);
      setEditing(false);
      setEditValue("");
      setEditLabel("");
      onCredentialsChanged?.();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleRemove = async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await removeApiKey(providerId);
      setSaved(next.find((k) => k.providerId === providerId) ?? null);
      onCredentialsChanged?.();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="provider-detail-section">
      <h4>API Key</h4>

      {error && (
        <div className="settings-status settings-status--error">{error}</div>
      )}

      <ul className="credential-list">
        <li className="credential-card">
          <div className="credential-card__header">
            <div className="credential-card__info">
              <span className="credential-card__meta">
                {saved ? (
                  <>
                    <span className="credential-card__badge credential-card__badge--set">
                      Configured
                    </span>
                    <span className="credential-card__masked">
                      {saved.maskedKey}
                    </span>
                    {saved.label && (
                      <span className="credential-card__label">
                        {saved.label}
                      </span>
                    )}
                    <span className="credential-card__date">
                      Saved {saved.savedAt}
                    </span>
                  </>
                ) : (
                  <span className="credential-card__badge credential-card__badge--unset">
                    Not set
                  </span>
                )}
              </span>
            </div>
            <div className="credential-card__actions">
              {!editing && (
                <button
                  className="credential-btn"
                  disabled={busy}
                  onClick={() => {
                    setEditing(true);
                    setEditValue("");
                    setEditLabel(saved?.label ?? "");
                  }}
                >
                  {saved ? "Update" : "Add Key"}
                </button>
              )}
              {saved && !editing && (
                <button
                  className="credential-btn credential-btn--danger"
                  disabled={busy}
                  onClick={() => void handleRemove()}
                >
                  Remove
                </button>
              )}
            </div>
          </div>

          {info.help && !editing && (
            <p className="credential-card__help">{info.help}</p>
          )}

          {info.dashboardUrl && !editing && (
            <a
              className="credential-card__link"
              href={info.dashboardUrl}
              target="_blank"
              rel="noopener noreferrer"
            >
              Open dashboard ↗
            </a>
          )}

          {editing && (
            <div className="credential-card__edit">
              <input
                type="password"
                className="text-input credential-card__input"
                placeholder="Paste API key…"
                autoComplete="off"
                value={editValue}
                onChange={(e) => setEditValue(e.target.value)}
                disabled={busy}
              />
              <input
                type="text"
                className="text-input credential-card__input credential-card__input--label"
                placeholder="Label (optional)"
                value={editLabel}
                onChange={(e) => setEditLabel(e.target.value)}
                disabled={busy}
              />
              <div className="credential-card__edit-actions">
                <button
                  className="credential-btn credential-btn--primary"
                  disabled={busy || !editValue.trim()}
                  onClick={() => void handleSave()}
                >
                  Save
                </button>
                <button
                  className="credential-btn"
                  disabled={busy}
                  onClick={() => {
                    setEditing(false);
                    setEditValue("");
                    setEditLabel("");
                  }}
                >
                  Cancel
                </button>
              </div>
            </div>
          )}
        </li>
      </ul>
    </section>
  );
}
