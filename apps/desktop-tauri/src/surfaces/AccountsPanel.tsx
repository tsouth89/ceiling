import { useEffect, useState } from "react";
import type { ProviderUsageSnapshot } from "../types/bridge";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { capacityFreshness } from "../lib/capacityPresentation";
import { formatRelativeUpdated } from "../lib/relativeTime";
import { useLocale } from "../hooks/useLocale";
import { providerRowKey } from "../lib/providerRow";

/**
 * Accounts = "what is Ceiling watching, and is it healthy?" One card per
 * configured provider: brand, plan, account, where the data comes from, a
 * connection-health dot, and when it last updated. Clicking a card (or the
 * footer link) jumps to Settings → Providers to add or manage sources.
 */

type Status = "connected" | "stale" | "error";

function statusOf(provider: ProviderUsageSnapshot, nowMs: number): Status {
  const freshness = capacityFreshness(provider, nowMs);
  if (freshness === "error") return "error";
  if (freshness === "stale") return "stale";
  return "connected"; // live counts as connected
}

function normalizePlan(planName: string | null): string | null {
  if (!planName) return null;
  if (planName.trim().toLowerCase() === "default_claude_ai") return "Claude AI";
  return planName;
}

function maskEmail(email: string | null, hide: boolean): string | null {
  if (!email) return null;
  if (!hide) return email;
  return `${email[0]}•••`;
}

const STATUS_LABEL: Record<Status, string> = {
  connected: "Connected",
  stale: "Stale",
  error: "Error",
};

function AccountRow({
  provider,
  hideEmail,
  nowMs,
  onManage,
}: {
  provider: ProviderUsageSnapshot;
  hideEmail: boolean;
  nowMs: number;
  onManage: () => void;
}) {
  const { t } = useLocale();
  const status = statusOf(provider, nowMs);
  const plan = normalizePlan(provider.planName);
  const email = maskEmail(provider.accountEmail, hideEmail);
  const org = provider.accountOrganization;
  const updatedMs = Date.parse(provider.updatedAt);
  const updated = formatRelativeUpdated(
    Number.isNaN(updatedMs) ? null : updatedMs,
    t,
    nowMs,
  );

  return (
    <button type="button" className="account-card" onClick={onManage}>
      <ProviderIcon
        providerId={provider.providerId}
        size={30}
        className="account-card__icon"
        title={provider.displayName}
      />
      <div className="account-card__body">
        <div className="account-card__top">
          <span className="account-card__name">{provider.displayName}</span>
          {plan && <span className="account-card__plan">{plan}</span>}
        </div>
        <div className="account-card__meta">
          {email && <span className="account-card__email">{email}</span>}
          {org && <span className="account-card__org">{org}</span>}
          {provider.sourceLabel && (
            <span className="account-card__source">{provider.sourceLabel}</span>
          )}
        </div>
        {provider.error && (
          <div className="account-card__error">{provider.error}</div>
        )}
      </div>
      <div className="account-card__status">
        <span className="account-card__status-line">
          <span className="account-card__dot" data-status={status} aria-hidden />
          <span className="account-card__status-label">
            {STATUS_LABEL[status]}
          </span>
        </span>
        <span className="account-card__updated">{updated}</span>
      </div>
    </button>
  );
}

export default function AccountsPanel({
  providers,
  hideEmail,
  onManage,
}: {
  providers: ProviderUsageSnapshot[];
  hideEmail: boolean;
  onManage: () => void;
}) {
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, []);

  if (providers.length === 0) {
    return (
      <div className="accounts-empty">
        <strong>No accounts yet</strong>
        Add a provider in Settings and Ceiling will start tracking its limits.
        <button
          type="button"
          className="accounts-manage-link"
          onClick={onManage}
        >
          Manage providers
        </button>
      </div>
    );
  }

  return (
    <div className="accounts-panel">
      <div className="accounts-list">
        {providers.map((provider) => (
          <AccountRow
            key={providerRowKey(provider)}
            provider={provider}
            hideEmail={hideEmail}
            nowMs={nowMs}
            onManage={onManage}
          />
        ))}
      </div>
      <button type="button" className="accounts-manage-link" onClick={onManage}>
        Add or manage providers
      </button>
    </div>
  );
}
