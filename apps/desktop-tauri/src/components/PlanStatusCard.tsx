import type { CSSProperties } from "react";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";
import { ProviderIcon } from "./providers/ProviderIcon";
import { getProviderIcon } from "./providers/providerIcons";
import { useFormattedResetTime } from "../hooks/useFormattedResetTime";
import { useLocale } from "../hooks/useLocale";
import {
  activePromoBoosts,
  capacityFreshness,
  glanceMeters,
  type ConstrainingWindow,
} from "../lib/capacityPresentation";

function displayPlanName(planName: string | null): string | null {
  if (!planName) return null;
  const normalized = planName.trim().toLowerCase();
  if (normalized === "default_claude_ai") return "Claude AI";
  return planName;
}

function levelOf(remainPct: number, exhausted: boolean): string {
  if (exhausted) return "exhausted";
  if (remainPct <= 5) return "critical";
  if (remainPct <= 25) return "high";
  return "normal";
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
  const formattedReset = useFormattedResetTime(
    snap.resetsAt,
    snap.resetDescription,
    resetTimeRelative,
  );
  const showReset =
    !!formattedReset &&
    (!snap.isExhausted || showResetWhenExhausted);

  return (
    <div className={`plan-status-card__meter${hero ? " plan-status-card__meter--hero" : ""}`}>
      <div className="plan-status-card__meter-top">
        <span className="plan-status-card__meter-label">{meter.label}</span>
        <strong className="plan-status-card__meter-pct">
          {Math.round(displayPct)}% {suffix}
        </strong>
        {showReset && (
          <span className="plan-status-card__meter-reset">{formattedReset}</span>
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
  hideEmail,
  resetTimeRelative,
  showResetWhenExhausted = false,
  showAsUsed = false,
  isRefreshing = false,
  onSelect,
}: {
  provider: ProviderUsageSnapshot;
  hideEmail: boolean;
  resetTimeRelative: boolean;
  showResetWhenExhausted?: boolean;
  showAsUsed?: boolean;
  isRefreshing?: boolean;
  onSelect?: () => void;
}) {
  const brand = getProviderIcon(provider.providerId).brandColor;
  const meters = glanceMeters(provider);
  const freshness = capacityFreshness(provider);
  const promos = activePromoBoosts(provider);
  const planName = displayPlanName(provider.planName);
  const email =
    provider.accountEmail && !hideEmail
      ? provider.accountEmail
      : provider.accountEmail
        ? `${provider.accountEmail[0]}•••`
        : null;

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
            {planName && (
              <span className="plan-status-card__plan">{planName}</span>
            )}
          </div>
          <div className="plan-status-card__meta">
            {email && <span className="plan-status-card__email">{email}</span>}
            {freshness !== "live" && !provider.error && (
              <span
                className={`plan-status-card__chip plan-status-card__chip--${freshness}`}
              >
                {freshness}
              </span>
            )}
            {promos.map((promo) => (
              <span
                key={promo.id}
                className="plan-status-card__chip plan-status-card__chip--boost"
                title={promo.description}
              >
                {promo.title}
              </span>
            ))}
          </div>
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
          {meters.companion && (
            <MeterRow
              meter={meters.companion}
              showAsUsed={showAsUsed}
              resetTimeRelative={resetTimeRelative}
              showResetWhenExhausted={showResetWhenExhausted}
              hero={false}
            />
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
