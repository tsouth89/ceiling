import { useEffect, useMemo, useState } from "react";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { providerLocalUsageWindows } from "../lib/providerCharts";
import { getProviderChartData } from "../lib/tauri";
import type {
  LocalTokenBreakdown,
  LocalUsageComparisonPeriod,
  ProviderChartData,
  ProviderUsageSnapshot,
} from "../types/bridge";

/**
 * Formats a token count using compact US English notation.
 *
 * @param value - The token count to format
 * @returns The formatted token count
 */
function formatTokens(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

/**
 * Calculates the percentage of processed tokens associated with cache activity.
 *
 * @param breakdown - Token counts used to calculate the cache share.
 * @returns The cache activity percentage, or `0` when no tokens were processed.
 */
function cacheShare(breakdown: LocalTokenBreakdown): number {
  if (breakdown.processedTokens <= 0) return 0;
  return (((breakdown.cacheReadTokens + breakdown.cacheWriteTokens) /
    breakdown.processedTokens) * 100);
}

/**
 * Describes the change in token activity compared with the prior period.
 *
 * @param period - The current and previous token activity values.
 * @returns A percentage change relative to the prior period, or a message for periods with no previous activity.
 */
function periodChange(period: LocalUsageComparisonPeriod): string {
  if (period.previousTokens <= 0) {
    return period.currentTokens > 0 ? "New activity" : "No change";
  }
  const change = ((period.currentTokens - period.previousTokens) / period.previousTokens) * 100;
  const sign = change > 0 ? "+" : "";
  return `${sign}${Math.round(change)}% vs prior`;
}

/**
 * Summarizes which provider processed more local activity.
 *
 * @param leftName - Display name of the first provider
 * @param leftTokens - Processed token count for the first provider
 * @param rightName - Display name of the second provider
 * @param rightTokens - Processed token count for the second provider
 * @returns A summary indicating whether activity was even, one provider recorded all activity, or the leading provider processed more tokens
 */
function comparisonSummary(leftName: string, leftTokens: number, rightName: string, rightTokens: number): string {
  if (leftTokens === rightTokens) return "Even activity across both providers";
  const [leaderName, leaderTokens, otherTokens] = leftTokens > rightTokens
    ? [leftName, leftTokens, rightTokens]
    : [rightName, rightTokens, leftTokens];
  if (otherTokens <= 0) return `${leaderName} recorded all local activity`;
  return `${leaderName} processed ${(leaderTokens / otherTokens).toFixed(1)}× more`;
}

/**
 * Finds a local usage comparison period by identifier.
 *
 * @param data - Provider chart data containing the comparison periods
 * @param periodId - Identifier of the period to find
 * @returns The matching comparison period, or `null` when unavailable
 */
function providerPeriod(data: ProviderChartData | null, periodId: string): LocalUsageComparisonPeriod | null {
  return data?.localUsage?.comparisonPeriods?.find((period) => period.id === periodId) ?? null;
}

/**
 * Renders a provider usage comparison card for a rolling time window.
 *
 * @param periodId - Identifier of the comparison period to display
 * @param label - Display label for the comparison period
 * @param providers - The two providers being compared
 * @param data - Provider chart data keyed by provider identifier
 * @returns A comparison card, or an unavailable-history message when either provider lacks the period
 */
function ComparisonCard({ periodId, label, providers, data }: {
  periodId: string;
  label: string;
  providers: [ProviderUsageSnapshot, ProviderUsageSnapshot];
  data: Record<string, ProviderChartData | null>;
}) {
  const rows = providers.map((provider) => ({ provider, period: providerPeriod(data[provider.providerId], periodId) }));
  if (rows.some((row) => row.period === null)) {
    return (
      <section className="provider-comparison-card provider-comparison-card--empty">
        <strong>{label}</strong>
        <span>Comparable local history is not available for both providers yet.</span>
      </section>
    );
  }
  const complete = rows as Array<{ provider: ProviderUsageSnapshot; period: LocalUsageComparisonPeriod }>;
  const maxTokens = Math.max(1, ...complete.map((row) => row.period.currentTokens));

  return (
    <section className="provider-comparison-card">
      <header className="provider-comparison-card__header">
        <div><strong>{label}</strong><span>Same rolling window</span></div>
        <span className="provider-comparison-card__summary">
          {comparisonSummary(complete[0].provider.displayName, complete[0].period.currentTokens, complete[1].provider.displayName, complete[1].period.currentTokens)}
        </span>
      </header>
      <div className="provider-comparison-card__rows">
        {complete.map(({ provider, period }) => (
          <div className="provider-comparison-row" key={provider.providerId}>
            <ProviderIcon providerId={provider.providerId} size={24} className="provider-comparison-row__icon" title={provider.displayName} />
            <div className="provider-comparison-row__body">
              <div className="provider-comparison-row__top"><strong>{provider.displayName}</strong><span>{formatTokens(period.currentTokens)}</span></div>
              <div className="provider-comparison-row__track" aria-hidden="true"><span style={{ width: `${(period.currentTokens / maxTokens) * 100}%` }} /></div>
              <div className="provider-comparison-row__details">
                <span>{periodChange(period)}</span>
                <span>{formatTokens(period.currentBreakdown.outputTokens)} output</span>
                <span>{cacheShare(period.currentBreakdown).toFixed(1)}% cache</span>
              </div>
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}

/**
 * Compares local token usage for two providers across matching rolling windows.
 *
 * Displays loading, timeout, and comparison states while retrieving and polling
 * provider usage data.
 *
 * @param providers - The two providers to compare
 */
export default function ProviderComparison({ providers }: {
  providers: [ProviderUsageSnapshot, ProviderUsageSnapshot];
}) {
  const [data, setData] = useState<Record<string, ProviderChartData | null>>({});
  const [loading, setLoading] = useState(true);
  const [timedOut, setTimedOut] = useState(false);
  const [reloadNonce, setReloadNonce] = useState(0);
  const requestKey = useMemo(() => providers.map((provider) => {
    const windows = providerLocalUsageWindows(provider);
    return `${provider.providerId}:${windows.map((window) => window.startsAt).join(",")}`;
  }).join("|"), [providers]);

  useEffect(() => {
    let cancelled = false;
    let attempts = 0;
    let timer: number | null = null;
    const scheduleRetry = () => {
      if (cancelled) return;
      if (attempts >= 60) {
        setLoading(false);
        setTimedOut(true);
        return;
      }
      attempts += 1;
      timer = window.setTimeout(() => void load(), 1_000);
    };
    const load = async () => {
      try {
        const results = await Promise.all(providers.map(async (provider) => {
          const result = await getProviderChartData(provider.providerId, provider.accountEmail ?? undefined, providerLocalUsageWindows(provider));
          return [provider.providerId, result] as const;
        }));
        if (cancelled) return;
        setData(Object.fromEntries(results));
        const ready = results.every(([, result]) => (result.localUsage?.comparisonPeriods?.length ?? 0) >= 2);
        if (ready) {
          setLoading(false);
          setTimedOut(false);
          return;
        }
        scheduleRetry();
      } catch {
        // The Tauri dev backend can briefly disappear during a Rust rebuild,
        // and production requests may fail transiently. Keep the comparison
        // alive instead of abandoning its polling loop after one rejection.
        scheduleRetry();
      }
    };
    setLoading(true);
    setTimedOut(false);
    setData({});
    void load();
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [requestKey, reloadNonce]);

  if (loading) {
    return (
      <div className="provider-comparison-loading" role="status">
        <span className="charts-loading__pulse" aria-hidden="true" />
        <div><strong>Comparing local history</strong><span>Reading Codex and Claude activity in the same time windows.</span></div>
      </div>
    );
  }

  if (timedOut) {
    return (
      <div className="provider-comparison-loading provider-comparison-loading--stalled" role="status">
        <div>
          <strong>Local history is taking longer than expected</strong>
          <span>Quota charts are still available. Retry the local-log comparison when you’re ready.</span>
        </div>
        <button type="button" onClick={() => setReloadNonce((value) => value + 1)}>
          Retry
        </button>
      </div>
    );
  }

  return (
    <div className="provider-comparison">
      <div className="provider-comparison__intro">
        <div><strong>Codex and Claude, on the same clock</strong><span>Processed tokens from local logs across every agent.</span></div>
        <span className="provider-comparison__source">Local logs</span>
      </div>
      <ComparisonCard periodId="five-hours" label="Last 5 hours" providers={providers} data={data} />
      <ComparisonCard periodId="seven-days" label="Last 7 days" providers={providers} data={data} />
      <p className="provider-comparison__note">Processed tokens include fresh input, output, cache reads, and cache writes. They measure activity, not subscription allowance.</p>
    </div>
  );
}
