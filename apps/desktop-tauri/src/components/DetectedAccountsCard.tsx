import { useEffect, useMemo, useState } from "react";
import type { DetectedProviderAccount } from "../types/bridge";
import { getDetectedProviderAccounts } from "../lib/tauri";
import {
  getIgnoredDetectedProviderIds,
  setDetectedProviderIgnored,
} from "../lib/detectedProviderPreferences";
import { ProviderIcon } from "./providers/ProviderIcon";

const DISMISSED_SIGNATURE_KEY = "ceiling.detected-accounts.dismissed.v1";

function accountSignature(accounts: DetectedProviderAccount[]): string {
  return accounts
    .map((account) => `${account.providerId}:${account.status}`)
    .sort()
    .join("|");
}

export default function DetectedAccountsCard({
  enabledProviderIds,
  previouslyTrackedProviderIds = [],
  onEnable,
  onManage,
}: {
  enabledProviderIds: string[];
  previouslyTrackedProviderIds?: string[];
  onEnable: (providerIds: string[]) => Promise<void>;
  onManage: () => void;
}) {
  const [accounts, setAccounts] = useState<DetectedProviderAccount[] | null>(null);
  const [pendingIds, setPendingIds] = useState<Set<string>>(new Set());
  const [dismissedSignature, setDismissedSignature] = useState(() => {
    try {
      return window.localStorage.getItem(DISMISSED_SIGNATURE_KEY) ?? "";
    } catch {
      return "";
    }
  });

  useEffect(() => {
    let cancelled = false;
    getDetectedProviderAccounts()
      .then((detected) => {
        if (!cancelled) setAccounts(detected);
      })
      .catch(() => {
        if (!cancelled) setAccounts([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const candidates = useMemo(() => {
    const enabled = new Set(enabledProviderIds);
    const ignored = getIgnoredDetectedProviderIds();
    const previouslyTracked = new Set(previouslyTrackedProviderIds);
    return (accounts ?? []).filter(
      (account) =>
        !enabled.has(account.providerId) &&
        account.status !== "unavailable" &&
        !ignored.has(account.providerId) &&
        !previouslyTracked.has(account.providerId),
    );
  }, [accounts, enabledProviderIds, previouslyTrackedProviderIds]);

  useEffect(() => {
    const enabled = new Set(enabledProviderIds);
    previouslyTrackedProviderIds.forEach((providerId) => {
      if (!enabled.has(providerId)) setDetectedProviderIgnored(providerId, true);
    });
  }, [enabledProviderIds, previouslyTrackedProviderIds]);

  const signature = accountSignature(candidates);
  const ready = candidates.filter((account) => account.status === "ready");

  if (
    accounts === null ||
    candidates.length === 0 ||
    signature === dismissedSignature
  ) {
    return null;
  }

  const enable = async (providerIds: string[]) => {
    if (providerIds.length === 0) return;
    setPendingIds((current) => new Set([...current, ...providerIds]));
    try {
      await onEnable(providerIds);
      providerIds.forEach((providerId) => setDetectedProviderIgnored(providerId, false));
    } finally {
      setPendingIds((current) => {
        const next = new Set(current);
        providerIds.forEach((id) => next.delete(id));
        return next;
      });
    }
  };

  const dismiss = () => {
    try {
      window.localStorage.setItem(DISMISSED_SIGNATURE_KEY, signature);
    } catch {
      // Dismiss for this render even if WebView storage is unavailable.
    }
    setDismissedSignature(signature);
  };

  return (
    <section className="detected-accounts" aria-label="Available providers">
      <div className="detected-accounts__wash" aria-hidden />
      <header className="detected-accounts__header">
        <div className="detected-accounts__spark" aria-hidden>
          <svg viewBox="0 0 24 24">
            <path d="M12 2.8c.7 4.7 3.5 7.5 8.2 8.2-4.7.7-7.5 3.5-8.2 8.2-.7-4.7-3.5-7.5-8.2-8.2 4.7-.7 7.5-3.5 8.2-8.2Z" />
          </svg>
        </div>
        <div>
          <h2>Available to track</h2>
          <p>Tools found on this PC. Choose which ones Ceiling should track.</p>
        </div>
      </header>

      <div className="detected-accounts__list">
        {candidates.map((account) => {
          const isReady = account.status === "ready";
          const pending = pendingIds.has(account.providerId);
          return (
            <div className="detected-account" key={account.providerId}>
              <ProviderIcon providerId={account.providerId} size={28} />
              <div className="detected-account__identity">
                <strong>{account.displayName}</strong>
                <span>{account.sourceLabel}</span>
              </div>
              <div
                className={`detected-account__status detected-account__status--${account.status}`}
              >
                <span aria-hidden />
                {isReady
                  ? "Ready"
                  : account.status === "locked"
                    ? "App open"
                    : "Sign in needed"}
              </div>
              <button
                type="button"
                className={isReady ? "detected-account__track" : "detected-account__review"}
                disabled={pending}
                onClick={() => {
                  if (isReady) {
                    void enable([account.providerId]);
                  } else {
                    onManage();
                  }
                }}
              >
                {pending ? "Adding…" : isReady ? "Track" : "Review"}
              </button>
            </div>
          );
        })}
      </div>

      <footer className="detected-accounts__footer">
        <button type="button" className="detected-accounts__later" onClick={dismiss}>
          Not now
        </button>
        {ready.length > 1 && (
          <button
            type="button"
            className="detected-accounts__all"
            disabled={ready.some((account) => pendingIds.has(account.providerId))}
            onClick={() => void enable(ready.map((account) => account.providerId))}
          >
            Track all
          </button>
        )}
      </footer>
    </section>
  );
}
