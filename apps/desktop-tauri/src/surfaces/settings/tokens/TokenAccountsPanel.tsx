import { useCallback, useEffect, useState } from "react";
import type { ProviderTokenAccountsBridge } from "../../../types/bridge";
import { useLocale } from "../../../hooks/useLocale";
import {
  addTokenAccount,
  getTokenAccounts,
  removeTokenAccount,
  setActiveTokenAccount,
  triggerProviderLogin,
} from "../../../lib/tauri";
import { listen } from "@tauri-apps/api/event";
import {
  providerLoginPhaseKey,
  type ProviderLoginPhase,
  type ProviderLoginPhaseChangedPayload,
} from "../../../lib/providerLogin";

interface Props {
  providerId: string;
  /**
   * When `true`, renders a tight collapsible variant suitable for embedding
   * inside the Providers → detail pane (Phase 6e inline surface).
   * When `false` (default), renders the full standalone-tab layout.
   */
  compact?: boolean;
}

/**
 * Shared Token Accounts surface.
 *
 * Port of the token-account blocks in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 * Wires the existing Phase-4 IPC (`get_token_accounts`,
 * `add_token_account`, `remove_token_account`, `set_active_token_account`).
 *
 * Used by:
 *   - `Settings.tsx::TokenAccountsTab` (compact=false, standalone tab)
 *   - `ProviderDetailPane.tsx` (compact=true, inline in detail pane)
 */
export function TokenAccountsPanel({ providerId, compact = false }: Props) {
  const { t } = useLocale();
  const [data, setData] = useState<ProviderTokenAccountsBridge | null>(null);
  const [busy, setBusy] = useState(false);
  const [loginPhase, setLoginPhase] = useState<ProviderLoginPhase | null>(null);
  const [loginCode, setLoginCode] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [addLabel, setAddLabel] = useState("");
  const [addToken, setAddToken] = useState("");

  const load = useCallback(async () => {
    if (!providerId) {
      setData(null);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await getTokenAccounts(providerId);
      setData(next);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
      setData(null);
    } finally {
      setBusy(false);
    }
  }, [providerId]);

  useEffect(() => {
    setAddLabel("");
    setAddToken("");
    setError(null);
    void load();
  }, [load]);

  useEffect(() => {
    setLoginPhase(null);
    setLoginCode(null);
    const unlistenPromise = listen<ProviderLoginPhaseChangedPayload>(
      "login-phase-changed",
      ({ payload }) => {
        if (payload.providerId === providerId) {
          setLoginPhase(payload.phase);
          setLoginCode(payload.code ?? null);
        }
      },
    );
    return () => {
      void unlistenPromise.then((fn) => fn());
    };
  }, [providerId]);

  const handleAdd = async () => {
    if (!providerId || !addLabel.trim() || !addToken.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const next = await addTokenAccount(
        providerId,
        addLabel.trim(),
        addToken.trim(),
      );
      setData(next);
      setAddLabel("");
      setAddToken("");
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleRemove = async (accountId: string) => {
    if (!providerId) return;
    setBusy(true);
    setError(null);
    try {
      const next = await removeTokenAccount(providerId, accountId);
      setData(next);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleSetActive = async (accountId: string) => {
    if (!providerId) return;
    setBusy(true);
    setError(null);
    try {
      const next = await setActiveTokenAccount(providerId, accountId);
      setData(next);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleProviderLogin = async () => {
    if (!providerId) return;
    setBusy(true);
    setLoginPhase(null);
    setLoginCode(null);
    setError(null);
    try {
      await triggerProviderLogin(providerId);
      await load();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  if (!providerId) return null;

  const placeholder = data?.support.placeholder ?? "Paste token…";
  const subtitle = data?.support.subtitle ?? "";
  const loginStatusKey = providerLoginPhaseKey(loginPhase);

  const body = (
    <>
      {subtitle && !compact && (
        <p className="settings-section__hint">{subtitle}</p>
      )}

      {error && (
        <div className="settings-status settings-status--error">{error}</div>
      )}
      {loginStatusKey && (
        <div className="settings-status" role="status">
          {t(loginStatusKey)}
          {loginPhase === "waitingBrowser" && loginCode && (
            <>
              {" "}
              {t("LoginPhaseEnterGithubCodePrefix")}{" "}
              <strong className="settings-status__code">{loginCode}</strong>
            </>
          )}
        </div>
      )}

      {!compact && (
        <h3 className="settings-section__title">
          {t("SectionSavedAccounts")}
        </h3>
      )}

      {data && data.accounts.length > 0 ? (
        <ul className="credential-list token-accounts-list">
          {data.accounts.map((acct) => (
            <li key={acct.id} className="credential-card token-accounts-card">
              <div className="credential-card__header">
                <div className="credential-card__info">
                  <strong>{acct.label}</strong>
                  <span className="credential-card__meta">
                    {acct.isActive && (
                      <span className="credential-card__badge credential-card__badge--set">
                        {t("TokenAccountActive")}
                      </span>
                    )}
                    <span className="credential-card__date">
                      {t("TokenAccountAddedPrefix")} {acct.addedAt}
                    </span>
                    {acct.lastUsed && (
                      <span className="credential-card__date">
                        · {t("TokenAccountUsedPrefix")} {acct.lastUsed}
                      </span>
                    )}
                  </span>
                </div>
                <div className="credential-card__actions">
                  {!acct.isActive && (
                    <button
                      className="credential-btn credential-btn--secondary"
                      disabled={busy}
                      onClick={() => void handleSetActive(acct.id)}
                    >
                      {t("TokenAccountSetActive")}
                    </button>
                  )}
                  <button
                    className="credential-btn credential-btn--danger"
                    disabled={busy}
                    onClick={() => void handleRemove(acct.id)}
                  >
                    {t("TokenAccountRemove")}
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <p className="credential-empty">{t("TokenAccountEmpty")}</p>
      )}

      {!compact && (
        <h3 className="settings-section__title">{t("SectionAddAccount")}</h3>
      )}
      {providerId === "copilot" && (
        <button
          className="credential-btn credential-btn--primary"
          disabled={busy}
          onClick={() => void handleProviderLogin()}
        >
          {loginStatusKey && busy
            ? t(loginStatusKey)
            : t("TokenAccountGithubLoginButton")}
        </button>
      )}
      <div className="credential-add-form token-accounts-add">
        <input
          className="text-input"
          type="text"
          placeholder={t("TokenAccountLabelPlaceholder")}
          value={addLabel}
          onChange={(e) => setAddLabel(e.target.value)}
          disabled={busy}
        />
        <textarea
          className="text-input credential-textarea"
          placeholder={placeholder}
          rows={compact ? 2 : 3}
          value={addToken}
          onChange={(e) => setAddToken(e.target.value)}
          disabled={busy}
        />
        <button
          className="credential-btn credential-btn--primary"
          disabled={busy || !addLabel.trim() || !addToken.trim()}
          onClick={() => void handleAdd()}
        >
          {t("TokenAccountAddButton")}
        </button>
      </div>
    </>
  );

  if (compact) {
    const activeCount = data?.accounts.filter((a) => a.isActive).length ?? 0;
    const total = data?.accounts.length ?? 0;
    return (
      <details className="provider-detail-section token-accounts-inline">
        <summary className="token-accounts-inline__summary">
          <span>{t("TokenAccountInlineSummary")}</span>
          <span className="token-accounts-inline__count">
            {activeCount > 0 ? `${activeCount}/${total}` : `${total}`}
          </span>
        </summary>
        <div className="token-accounts-inline__body">{body}</div>
      </details>
    );
  }

  return (
    <section className="settings-section token-accounts-standalone">
      <h3 className="settings-section__title">{t("SectionTokenAccounts")}</h3>
      {body}
    </section>
  );
}
