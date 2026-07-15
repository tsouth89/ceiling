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
import { allMeasuredWindows, resetCreditsAvailable } from "../lib/capacityPresentation";

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

/**
 * Formats a provider plan name for display.
 *
 * @param planName - The provider's plan name, or `null` when unavailable
 * @returns The formatted plan name, or `null` when no plan name is provided
 */
function displayPlanName(planName: string | null): string | null {
  if (!planName) return null;
  if (planName.trim().toLowerCase() === "default_claude_ai") return "Claude AI";
  return planName;
}

/**
 * Formats a positive count using compact notation.
 *
 * @param value - The count to format
 * @returns A compactly formatted count, or `—` when the value is null or less than or equal to zero
 */
function formatCompactCount(value: number | null): string {
  if (value == null || value <= 0) return "—";
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: value >= 1_000_000 ? 1 : 0,
  }).format(value);
}

/**
 * Classifies a rate window by its remaining usage.
 *
 * @param window - The rate window snapshot to classify
 * @returns `"exhausted"` when exhausted, `"critical"` when 5% or less remains, or `"normal"` otherwise
 */
function usageLevel(window: RateWindowSnapshot): string {
  if (window.isExhausted) return "exhausted";
  if (window.remainingPercent <= 5) return "critical";
  return "normal";
}

/**
 * Converts a pacing stage into a human-readable budget status label.
 *
 * @param stage - The pacing stage to describe
 * @returns The corresponding budget status label
 */
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

/**
 * Classifies pacing as a risk or calm state based on its deviation percentage.
 *
 * @param pace - The pacing snapshot to classify
 * @returns `"risk"` if the deviation exceeds 5 percent, `"calm"` otherwise
 */
function paceTone(pace: PaceSnapshot): "risk" | "calm" {
  return pace.deltaPercent > 5 ? "risk" : "calm";
}

/**
 * Calculates the displayed percentage for a rate window as used or remaining capacity.
 *
 * @param window - The rate window containing the used percentage.
 * @param showAsUsed - Whether to display used capacity instead of remaining capacity.
 * @returns The clamped used or remaining percentage.
 */
function percentFor(window: RateWindowSnapshot, showAsUsed: boolean): number {
  const used = Math.max(0, Math.min(100, window.usedPercent));
  return showAsUsed ? used : 100 - used;
}

/**
 * Renders a progress bar for a rate window's used or remaining percentage.
 *
 * @param window - The rate window snapshot used to determine progress and status.
 * @param showAsUsed - Whether the bar represents usage rather than remaining capacity.
 */
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

/**
 * Renders a secondary usage limit with its percentage, reset information, and progress bar.
 *
 * @param metric - The labeled usage window to display
 * @param resetTimeRelative - Whether to format the reset time relatively
 * @param showAsUsed - Whether to display the percentage as used rather than remaining
 */
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

/**
 * Displays local token usage statistics, cache share, and the most-used model when available.
 *
 * @param summary - Local usage data used to populate the activity statistics.
 */
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

/**
 * Displays expected and actual usage progress for a provider's current pacing window.
 *
 * @param pace - Pacing data used to determine the status label and progress values
 */
function PaceSection({ pace }: { pace: PaceSnapshot }) {
  const tone = paceTone(pace);
  const expected = Math.max(0, Math.min(100, pace.expectedUsedPercent));
  const actual = Math.max(0, Math.min(100, pace.actualUsedPercent));
  return (
    <section className="provider-focus__section provider-focus__pace">
      <div className="provider-focus__section-head">
        <h3>Pace</h3>
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

/**
 * Displays a provider's usage, limits, pacing status, and available local activity.
 *
 * @param provider - Provider details and usage data to display
 * @param resetTimeRelative - Whether reset times should use relative formatting
 * @param showAsUsed - Whether usage values should be displayed as used rather than remaining
 * @param isRefreshing - Whether the provider data is currently refreshing
 */
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
  const resetCredits = resetCreditsAvailable(provider);
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
            <span className="provider-focus__reset-credit">
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
                <strong>{paceLabel(provider.pace.stage)}</strong>
                <span>
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
