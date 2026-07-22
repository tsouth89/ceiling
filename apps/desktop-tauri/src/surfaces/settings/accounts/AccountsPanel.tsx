import { useCallback, useEffect, useState } from "react";
import type {
  AccountProbeBridge,
  ProviderAccountsBridge,
} from "../../../types/bridge";
import { useLocale } from "../../../hooks/useLocale";
import {
  addDirectoryAccount,
  getDirectoryAccounts,
  probeAccountDirectory,
  removeDirectoryAccount,
  setActiveDirectoryAccount,
} from "../../../lib/tauri";

/** CLI to invoke when telling the user how to sign a second account in. */
const LOGIN_CLI: Record<string, string> = {
  codex: "codex",
  claude: "claude",
};

/**
 * Config-directory accounts (SOU-285).
 *
 * An account here is a provider config directory, not a stored token: the CLI
 * rotates its credential in place, so Ceiling points at the directory and lets
 * the CLI keep it fresh.
 *
 * With no accounts configured a provider stays in its default state, following
 * whichever account its CLI is signed in as. That is rendered as its own state
 * rather than as an empty list, because an empty list reads as broken.
 */
export function AccountsPanel() {
  const { t } = useLocale();
  const [providers, setProviders] = useState<ProviderAccountsBridge[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setProviders(await getDirectoryAccounts());
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // Every mutation returns that provider's new snapshot, so splice it in rather
  // than refetching everything.
  const applyResult = (next: ProviderAccountsBridge) => {
    setProviders((current) =>
      current.map((entry) =>
        entry.providerId === next.providerId ? next : entry,
      ),
    );
  };

  const run = async (op: () => Promise<ProviderAccountsBridge>) => {
    setBusy(true);
    setError(null);
    try {
      applyResult(await op());
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="settings-section accounts-standalone">
      <h3 className="settings-section__title">{t("SectionAccounts")}</h3>
      <p className="settings-section__hint">{t("AccountsIntro")}</p>

      {error && (
        <div className="settings-status settings-status--error">{error}</div>
      )}

      {providers.map((provider) => (
        <ProviderAccounts
          key={provider.providerId}
          provider={provider}
          busy={busy}
          onRun={run}
        />
      ))}
    </section>
  );
}

interface ProviderProps {
  provider: ProviderAccountsBridge;
  busy: boolean;
  onRun: (op: () => Promise<ProviderAccountsBridge>) => Promise<void>;
}

function ProviderAccounts({ provider, busy, onRun }: ProviderProps) {
  const { t } = useLocale();
  const [dir, setDir] = useState("");
  const [label, setLabel] = useState("");
  const [probe, setProbe] = useState<AccountProbeBridge | null>(null);
  const [probeError, setProbeError] = useState<string | null>(null);

  const resetForm = () => {
    setDir("");
    setLabel("");
    setProbe(null);
    setProbeError(null);
  };

  const handleProbe = async () => {
    if (!dir.trim()) return;
    setProbeError(null);
    try {
      const result = await probeAccountDirectory(provider.providerId, dir);
      setProbe(result);
      // Seed the label from the directory so the common case needs no typing.
      if (!label.trim() && result.suggestedLabel) setLabel(result.suggestedLabel);
    } catch (err: unknown) {
      setProbe(null);
      setProbeError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleAdd = async () => {
    if (!dir.trim()) return;
    await onRun(() =>
      addDirectoryAccount(
        provider.providerId,
        dir.trim(),
        label.trim() || null,
      ),
    );
    resetForm();
  };

  const cli = LOGIN_CLI[provider.providerId] ?? provider.providerId;

  return (
    <div className="accounts-provider">
      <h4 className="accounts-provider__title">{provider.displayName}</h4>

      {provider.followingCli ? (
        <div className="accounts-following">
          <span className="credential-card__badge credential-card__badge--set">
            {t("AccountsFollowingCli")}
          </span>
          <span className="credential-card__meta">
            {t("AccountsFollowingCliHint")}{" "}
            <code className="accounts-path">{provider.ambientDir}</code>
          </span>
        </div>
      ) : (
        <ul className="credential-list accounts-list">
          {provider.accounts.map((account) => (
            <li
              key={account.id}
              className="credential-card accounts-card"
              // A tint is validated as a plain hex color before it is stored.
              style={
                account.tint
                  ? { borderInlineStartColor: account.tint }
                  : undefined
              }
            >
              <div className="credential-card__header">
                <div className="credential-card__info">
                  <strong>{account.label}</strong>
                  <span className="credential-card__meta">
                    {account.isActive && (
                      <span className="credential-card__badge credential-card__badge--set">
                        {t("AccountsActive")}
                      </span>
                    )}
                    {!account.signedIn && (
                      <span className="credential-card__badge credential-card__badge--warn">
                        {t("AccountsSignedOut")}
                      </span>
                    )}
                    {account.organization && (
                      <span className="credential-card__date">
                        {account.organization}
                      </span>
                    )}
                    {account.plan && (
                      <span className="credential-card__date">
                        · {account.plan}
                      </span>
                    )}
                  </span>
                  <span className="credential-card__path">
                    {account.configDir}
                  </span>
                </div>
                <div className="credential-card__actions">
                  {!account.isActive && (
                    <button
                      className="credential-btn credential-btn--secondary"
                      disabled={busy}
                      onClick={() =>
                        void onRun(() =>
                          setActiveDirectoryAccount(
                            provider.providerId,
                            account.id,
                          ),
                        )
                      }
                    >
                      {t("AccountsTrackThis")}
                    </button>
                  )}
                  <button
                    className="credential-btn credential-btn--danger"
                    disabled={busy}
                    onClick={() =>
                      void onRun(() =>
                        removeDirectoryAccount(
                          provider.providerId,
                          account.id,
                        ),
                      )
                    }
                  >
                    {t("AccountsRemove")}
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}

      <p className="settings-section__hint accounts-setup-hint">
        {t("AccountsSetupHint")}{" "}
        <code className="accounts-path">
          $env:{provider.envVar}=&quot;&lt;path&gt;&quot;; {cli} login
        </code>
      </p>
      <div className="credential-add-form accounts-add">
        <input
          className="text-input"
          type="text"
          placeholder={t("AccountsDirPlaceholder")}
          value={dir}
          onChange={(e) => {
            setDir(e.target.value);
            setProbe(null);
          }}
          disabled={busy}
        />
        <input
          className="text-input"
          type="text"
          placeholder={t("AccountsLabelPlaceholder")}
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          disabled={busy}
        />
        <div className="accounts-add__actions">
          <button
            className="credential-btn credential-btn--secondary"
            disabled={busy || !dir.trim()}
            onClick={() => void handleProbe()}
          >
            {t("AccountsCheckButton")}
          </button>
          <button
            className="credential-btn credential-btn--primary"
            disabled={busy || !dir.trim()}
            onClick={() => void handleAdd()}
          >
            {t("AccountsAddButton")}
          </button>
        </div>

        {probeError && (
          <p className="settings-status settings-status--error">{probeError}</p>
        )}
        {probe && (
          <p className="credential-card__meta accounts-probe">
            {probe.alreadyAddedAs
              ? `${t("AccountsProbeAlreadyAdded")} ${probe.alreadyAddedAs}`
              : probe.signedIn
                ? (probe.suggestedLabel ?? probe.configDir)
                : t("AccountsProbeSignedOut")}
          </p>
        )}
      </div>

    </div>
  );
}
