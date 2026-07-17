import { useEffect, useMemo, useState } from "react";
import type {
  PaceSnapshot,
  ProviderChartData,
  ProviderLocalUsageSummary,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { useFormattedResetTime } from "../hooks/useFormattedResetTime";
import { useLocale } from "../hooks/useLocale";
import { formatRelativeUpdated } from "../lib/relativeTime";
import { getProviderChartData } from "../lib/tauri";
import { providerSupportsChartData } from "../lib/providerCharts";
import { allMeasuredWindows, codexResetCredits } from "../lib/capacityPresentation";

type DetailWindow = {
  id: string;
  label: string;
  window: RateWindowSnapshot;
};

interface ProviderDetailViewProps {
  provider: ProviderUsageSnapshot;
  resetTimeRelative: boolean;
  showAsUsed: boolean;
  isRefreshing?: boolean;
}

function displayPlanName(planName: string | null): string | null {
  if (!planName) return null;
  if (planName.trim().toLowerCase() === "default_claude_ai") return "Claude AI";
  return planName;
}

function formatCompactCount(value: number | null): string {
  if (value == null || value <= 0) return "—";
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: value >= 1_000_000 ? 1 : 0,
  }).format(value);
}

function usageLevel(window: RateWindowSnapshot): string {
  if (window.isExhausted) return "exhausted";
  if (window.remainingPercent <= 5) return "critical";
  return "normal";
}

function paceLabel(stage: PaceSnapshot["stage"]): string {
  switch (stage) {
    case "far_ahead":
      return "Far ahead of budget";
    case "ahead":
      return "Ahead of budget";
    case "slightly_ahead":
      return "Slightly ahead of budget";
    case "far_behind":
      return "Far below budget";
    case "behind":
      return "Below budget";
    case "slightly_behind":
      return "Slightly below budget";
    default:
      return "On pace";
  }
}

function paceTone(pace: PaceSnapshot): "risk" | "calm" {
  return pace.deltaPercent > 5 ? "risk" : "calm";
}

function percentFor(window: RateWindowSnapshot, showAsUsed: boolean): number {
  const used = Math.max(0, Math.min(100, window.usedPercent));
  return showAsUsed ? used : 100 - used;
}

function DetailProgress({
  window,
  showAsUsed,
}: {
  window: RateWindowSnapshot;
  showAsUsed: boolean;
}) {
  return (
    <div className="provider-focus__progress" aria-hidden>
      <div
        className="provider-focus__progress-fill"
        data-level={usageLevel(window)}
        style={{ width: `${percentFor(window, showAsUsed)}%` }}
      />
    </div>
  );
}

function SecondaryWindow({
  metric,
  resetTimeRelative,
  showAsUsed,
}: {
  metric: DetailWindow;
  resetTimeRelative: boolean;
  showAsUsed: boolean;
}) {
  const reset = useFormattedResetTime(
    metric.window.resetsAt,
    metric.window.resetDescription,
    resetTimeRelative,
  );
  const percent = Math.round(percentFor(metric.window, showAsUsed));
  return (
    <div className="provider-focus__limit-row">
      <div className="provider-focus__limit-head">
        <strong>{metric.label}</strong>
        <span className="provider-focus__limit-value">
          <b>{percent}%</b> {showAsUsed ? "used" : "left"}
        </span>
        {reset && <span className="provider-focus__limit-reset">{reset}</span>}
      </div>
      <DetailProgress window={metric.window} showAsUsed={showAsUsed} />
    </div>
  );
}

function LocalActivity({ summary }: { summary: ProviderLocalUsageSummary }) {
  const breakdown = summary.sevenDayTokenBreakdown;
  const cached = breakdown
    ? breakdown.cacheReadTokens + breakdown.cacheWriteTokens
    : 0;
  const cacheShare =
    breakdown && breakdown.processedTokens > 0
      ? `${((cached / breakdown.processedTokens) * 100).toFixed(1)}%`
      : "—";
  const stats = [
    ["Last session", formatCompactCount(summary.lastSessionTokens), "processed"],
    ["Last 7 days", formatCompactCount(summary.sevenDayTokens), "processed"],
    ["Last 30 days", formatCompactCount(summary.thirtyDayTokens), "processed"],
    ["Cache share", cacheShare, ""],
  ];
  return (
    <section className="provider-focus__section">
      <h3>Local activity</h3>
      <div className="provider-focus__stats">
        {stats.map(([label, value, suffix]) => (
          <div key={label}>
            <span>{label}</span>
            <strong>{value}</strong>
            {suffix && <small>{suffix}</small>}
          </div>
        ))}
      </div>
      <div className="provider-focus__source-note">
        Local logs
        {summary.topModel && <> · Most used model: {summary.topModel}</>}
      </div>
    </section>
  );
}

function PaceSection({ pace }: { pace: PaceSnapshot }) {
  const tone = paceTone(pace);
  const expected = Math.max(0, Math.min(100, pace.expectedUsedPercent));
  const actual = Math.max(0, Math.min(100, pace.actualUsedPercent));
  return (
    <section className="provider-focus__section provider-focus__pace">
      <div className="provider-focus__section-head">
        <h3>{pace.windowLabel} pace</h3>
        <span data-tone={tone}>
          {paceLabel(pace.stage)} ({pace.deltaPercent >= 0 ? "+" : ""}
          {pace.deltaPercent.toFixed(1)}%)
        </span>
      </div>
      <div className="provider-focus__pace-track" aria-hidden>
        <div
          className="provider-focus__pace-fill"
          data-tone={tone}
          style={{ width: `${actual}%` }}
        />
        <i style={{ left: `${expected}%` }} />
      </div>
      <div className="provider-focus__pace-legend">
        <span>Expected {Math.round(expected)}% by now</span>
        <span>Actual {Math.round(actual)}%</span>
      </div>
    </section>
  );
}

export default function ProviderDetailView({
  provider,
  resetTimeRelative,
  showAsUsed,
  isRefreshing = false,
}: ProviderDetailViewProps) {
  const { t } = useLocale();
  const [chartData, setChartData] = useState<ProviderChartData | null>(null);
  const supportsLocalActivity = providerSupportsChartData(provider.providerId);
  useEffect(() => {
    if (!supportsLocalActivity || provider.error) {
      setChartData(null);
      return;
    }
    let cancelled = false;
    setChartData(null);
    getProviderChartData(provider.providerId, provider.accountEmail ?? undefined)
      .then((data) => {
        if (!cancelled) setChartData(data);
      })
      .catch(() => {
        if (!cancelled) setChartData(null);
      });
    return () => {
      cancelled = true;
    };
  }, [provider.accountEmail, provider.error, provider.providerId, supportsLocalActivity]);

  const primaryReset = useFormattedResetTime(
    provider.primary.resetsAt,
    provider.primary.resetDescription,
    resetTimeRelative,
  );
  const metrics = useMemo(
    () =>
      allMeasuredWindows(provider)
        .slice(1)
        .filter((metric) => !/promotional|on-demand/i.test(metric.label)),
    [provider],
  );
  const primaryPercent = Math.round(percentFor(provider.primary, showAsUsed));
  const primaryLabel = provider.primaryLabel?.trim() || "Primary";
  const planName = displayPlanName(provider.planName);
  const resetCredits = codexResetCredits(provider);
  const updated = Number.isNaN(Date.parse(provider.updatedAt))
    ? provider.updatedAt
    : formatRelativeUpdated(Date.parse(provider.updatedAt), t);
  const updatedLabel = /^updated\b/i.test(updated) ? updated : `Updated ${updated}`;

  return (
    <article className="provider-focus" aria-busy={isRefreshing}>
      <header className="provider-focus__identity">
        <ProviderIcon
          providerId={provider.providerId}
          size={42}
          className="provider-focus__icon"
          title={provider.displayName}
        />
        <div>
          <h2>{provider.displayName}</h2>
          <span>{updatedLabel}</span>
        </div>
        <div className="provider-focus__badges">
          {planName && <span className="provider-focus__plan">{planName}</span>}
          {resetCredits != null && (
            <span
              className={`provider-focus__reset-credit${resetCredits === 0 ? " provider-focus__reset-credit--empty" : ""}`}
            >
              ↻ {resetCredits} {resetCredits === 1 ? "reset available" : "resets available"}
            </span>
          )}
        </div>
      </header>

      {provider.error ? (
        <section className="provider-focus__error">
          <strong>Usage unavailable</strong>
          <span>{provider.error}</span>
        </section>
      ) : (
        <>
          <section className="provider-focus__primary">
            <div className="provider-focus__section-head">
              <h3>{primaryLabel} usage</h3>
              {primaryReset && <span>{primaryReset}</span>}
            </div>
            <div className="provider-focus__primary-value">
              <strong>{primaryPercent}%</strong>
              <span>{showAsUsed ? "used" : "left"}</span>
            </div>
            <DetailProgress window={provider.primary} showAsUsed={showAsUsed} />
            {provider.pace && (
              <div className="provider-focus__pace-glance" data-tone={paceTone(provider.pace)}>
                <i />
                <strong>{provider.pace.windowLabel} pace</strong>
                <span>
                  {paceLabel(provider.pace.stage)} ·{" "}
                  {provider.pace.deltaPercent >= 0 ? "+" : ""}
                  {provider.pace.deltaPercent.toFixed(1)}%
                </span>
              </div>
            )}
          </section>

          {(metrics.length > 0 || (provider.inactiveRateWindows?.length ?? 0) > 0) && (
            <section className="provider-focus__section provider-focus__limits">
              <h3>Other limits</h3>
              {metrics.map((metric) => (
                <SecondaryWindow
                  key={metric.id}
                  metric={metric}
                  resetTimeRelative={resetTimeRelative}
                  showAsUsed={showAsUsed}
                />
              ))}
              {(provider.inactiveRateWindows ?? []).map((metric) => (
                <div className="provider-focus__inactive" key={metric.id}>
                  <strong>{metric.title}</strong>
                  <span className="provider-focus__info" aria-hidden>i</span>
                  <span>Not currently enforced</span>
                </div>
              ))}
            </section>
          )}

          {supportsLocalActivity && chartData === null && (
            <section className="provider-focus__section provider-focus__local-loading">
              <h3>Local activity</h3>
              <span>Reading local logs…</span>
            </section>
          )}
          {chartData?.localUsage && <LocalActivity summary={chartData.localUsage} />}
          {provider.pace && <PaceSection pace={provider.pace} />}
        </>
      )}
    </article>
  );
}
