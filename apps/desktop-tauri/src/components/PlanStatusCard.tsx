import type { CSSProperties } from "react";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";
import { ProviderIcon } from "./providers/ProviderIcon";
import { accountIdentityLabel } from "../lib/providerRow";
import { getProviderIcon } from "./providers/providerIcons";
import { useFormattedResetTime } from "../hooks/useFormattedResetTime";
import { useLocale } from "../hooks/useLocale";
import {
  capacityFreshness,
  codexResetCredits,
  glanceMeters,
  type ConstrainingWindow,
} from "../lib/capacityPresentation";

function displayPlanName(
  planName: string | null,
  providerName: string,
): string | null {
  if (!planName) return null;
  const trimmed = planName.trim();
  const normalized = trimmed.toLowerCase();
  if (normalized === "default_claude_ai") return "Claude AI";
  const prefix = `${providerName.trim()} `;
  if (trimmed.toLowerCase().startsWith(prefix.toLowerCase())) {
    return trimmed.slice(prefix.length).trim() || trimmed;
  }
  return trimmed;
}

function levelOf(remainPct: number, exhausted: boolean): string {
  if (exhausted) return "exhausted";
  if (remainPct <= 5) return "critical";
  if (remainPct <= 25) return "high";
  return "normal";
}

function pressureLabel(level: string): string | null {
  if (level === "exhausted") return "Depleted";
  if (level === "critical") return "Almost out";
  if (level === "high") return "Near limit";
  return null;
}

function inactiveWindowSummary(
  provider: ProviderUsageSnapshot,
  state: "notEnforced" | "unavailable",
): string | null {
  const labels = [...new Set(
    (provider.inactiveRateWindows ?? [])
      .filter((window) => (window.state ?? "notEnforced") === state)
      .map((window) => window.title.trim())
      .filter(Boolean),
  )];
  if (labels.length === 0) return null;
  const visible = labels.slice(0, 2).join(", ");
  const remaining = labels.length - 2;
  return remaining > 0 ? `${visible} +${remaining}` : visible;
}

function MeterRow({
  meter,
  showAsUsed,
  resetTimeRelative,
  showResetWhenExhausted,
  hero,
}: {
  meter: ConstrainingWindow;
  showAsUsed: boolean;
  resetTimeRelative: boolean;
  showResetWhenExhausted: boolean;
  hero: boolean;
}) {
  const { t } = useLocale();
  const snap = meter.window;
  const usedPct = Math.max(0, Math.min(100, snap.usedPercent));
  const remain = Math.max(0, Math.min(100, snap.remainingPercent));
  const displayPct = showAsUsed ? usedPct : remain;
  const barPct = showAsUsed ? usedPct : remain;
  const suffix = showAsUsed ? t("PanelUsedSuffix") : t("PanelLeftSuffix");
  const level = levelOf(remain, snap.isExhausted);
  const status = pressureLabel(level);
  const formattedReset = useFormattedResetTime(
    snap.resetsAt,
    snap.resetDescription,
    resetTimeRelative,
  );
  // Overview always surfaces reset when known — at 100% that is the answer.
  const showReset = !!formattedReset;
  const awaitingReset = snap.isExhausted && showReset;
  // Optional setting: promote reset into the hero slot when depleted.
  const resetAsHero = awaitingReset && showResetWhenExhausted;

  return (
    <div
      className={[
        "plan-status-card__meter",
        hero ? "plan-status-card__meter--hero" : null,
        awaitingReset ? "plan-status-card__meter--awaiting-reset" : null,
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <div className="plan-status-card__meter-top">
        <span className="plan-status-card__meter-label">{meter.label}</span>
        {resetAsHero ? (
          <>
            <span className="plan-status-card__meter-pct plan-status-card__meter-pct--quiet">
              {Math.round(displayPct)}% {suffix}
            </span>
            {status && (
              <span className="plan-status-card__pressure" data-level={level}>
                <span aria-hidden />
                {status}
              </span>
            )}
            <strong className="plan-status-card__meter-reset plan-status-card__meter-reset--hero">
              {formattedReset}
            </strong>
          </>
        ) : (
          <>
            <strong className="plan-status-card__meter-pct">
              {Math.round(displayPct)}% {suffix}
            </strong>
            {status && (
              <span className="plan-status-card__pressure" data-level={level}>
                <span aria-hidden />
                {status}
              </span>
            )}
            {showReset && (
              <span
                className={`plan-status-card__meter-reset${
                  awaitingReset ? " plan-status-card__meter-reset--emphasis" : ""
                }`}
              >
                {formattedReset}
              </span>
            )}
          </>
        )}
      </div>
      <div className="plan-status-card__bar" aria-hidden>
        <div
          className="plan-status-card__bar-fill"
          data-level={level}
          style={{ width: `${barPct}%` }}
        />
      </div>
    </div>
  );
}

export default function PlanStatusCard({
  provider,
  resetTimeRelative,
  showResetWhenExhausted = false,
  showAsUsed = false,
  isRefreshing = false,
  showAccount = false,
  onSelect,
}: {
  provider: ProviderUsageSnapshot;
  resetTimeRelative: boolean;
  showResetWhenExhausted?: boolean;
  showAsUsed?: boolean;
  isRefreshing?: boolean;
  // True when this provider has more than one account. With one, the account
  // name is noise, so it stays hidden and the plan chip shows as before.
  showAccount?: boolean;
  onSelect?: () => void;
}) {
  const brand = getProviderIcon(provider.providerId).brandColor;
  const meters = glanceMeters(provider);
  const freshness = capacityFreshness(provider);
  const planName = displayPlanName(provider.planName, provider.displayName);
  // The account's email, shown only when several accounts share this provider.
  // It replaces the plan chip because it already carries the plan.
  const accountName = showAccount ? accountIdentityLabel(provider) : null;
  const notEnforcedSummary = inactiveWindowSummary(provider, "notEnforced");
  const unavailableSummary = inactiveWindowSummary(provider, "unavailable");
  const resetCredits = codexResetCredits(provider);

  const className = [
    "plan-status-card",
    "menu-card",
    provider.error ? "plan-status-card--error menu-card--error" : null,
    freshness === "stale" ? "plan-status-card--stale menu-card--stale" : null,
    isRefreshing ? "plan-status-card--refreshing menu-card--refreshing" : null,
    onSelect ? "plan-status-card--interactive" : null,
  ]
    .filter(Boolean)
    .join(" ");

  const body = (
    <>
      <header className="plan-status-card__header">
        <ProviderIcon
          providerId={provider.providerId}
          size={30}
          className="plan-status-card__icon"
          title={provider.displayName}
        />
        <div className="plan-status-card__identity">
          <div className="plan-status-card__title-row">
            <span className="plan-status-card__name">{provider.displayName}</span>
            {/* Two rows of the same provider are otherwise indistinguishable:
                both read just "Codex". The account name is what tells them
                apart, so it outranks the plan for the limited space here. */}
            {accountName ? (
              <span
                className="plan-status-card__account"
                style={
                  provider.accountTint
                    ? { color: provider.accountTint }
                    : undefined
                }
                title={accountName}
              >
                {accountName}
              </span>
            ) : (
              planName && (
                <span className="plan-status-card__plan">{planName}</span>
              )
            )}
          </div>
          {!provider.error && (freshness === "stale" || resetCredits != null) && (
            <div className="plan-status-card__meta">
              {freshness === "stale" && (
                <span
                  className={`plan-status-card__chip plan-status-card__chip--${freshness}`}
                >
                  {freshness}
                </span>
              )}
              {resetCredits != null && (
                <span
                  className={`plan-status-card__reset-credit${resetCredits === 0 ? " plan-status-card__reset-credit--empty" : ""}`}
                >
                  ↻ {resetCredits} {resetCredits === 1 ? "reset available" : "resets available"}
                </span>
              )}
            </div>
          )}
        </div>
      </header>

      {provider.error ? (
        <p className="plan-status-card__error">{provider.error}</p>
      ) : (
        <div className="plan-status-card__meters">
          <MeterRow
            meter={meters.primary}
            showAsUsed={showAsUsed}
            resetTimeRelative={resetTimeRelative}
            showResetWhenExhausted={showResetWhenExhausted}
            hero
          />
          {meters.companions.map((meter) => (
            <MeterRow
              key={meter.id}
              meter={meter}
              showAsUsed={showAsUsed}
              resetTimeRelative={resetTimeRelative}
              showResetWhenExhausted={showResetWhenExhausted}
              hero={false}
            />
          ))}
          {notEnforcedSummary && (
            <div className="plan-status-card__inactive">
              <span className="plan-status-card__inactive-mark" aria-hidden />
              <span className="plan-status-card__inactive-name">
                {notEnforcedSummary}
              </span>
              <span>not currently enforced</span>
            </div>
          )}
          {unavailableSummary && (
            <div className="plan-status-card__inactive plan-status-card__inactive--unavailable">
              <span className="plan-status-card__inactive-mark" aria-hidden />
              <span className="plan-status-card__inactive-name">
                {unavailableSummary}
              </span>
              <span>unavailable</span>
            </div>
          )}
        </div>
      )}
    </>
  );

  if (onSelect) {
    return (
      <button
        type="button"
        className={className}
        style={{ "--plan-brand": brand } as CSSProperties}
        onClick={onSelect}
        aria-label={provider.displayName}
        aria-busy={isRefreshing}
      >
        {body}
      </button>
    );
  }

  return (
    <article
      className={className}
      style={{ "--plan-brand": brand } as CSSProperties}
      aria-busy={isRefreshing}
    >
      {body}
    </article>
  );
}

/** Exported for tests — percent display helper. */
export function glanceDisplayPercent(
  snap: RateWindowSnapshot,
  showAsUsed: boolean,
): number {
  return Math.round(
    showAsUsed
      ? Math.max(0, Math.min(100, snap.usedPercent))
      : Math.max(0, Math.min(100, snap.remainingPercent)),
  );
}
