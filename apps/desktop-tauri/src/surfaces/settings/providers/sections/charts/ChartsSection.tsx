import { useEffect, useState } from "react";
import {
  getCursorModelActivity,
  getProviderChartData,
  getSettingsSnapshot,
} from "../../../../../lib/tauri";
import {
  providerLocalUsageWindows,
  providerSupportsChartData,
} from "../../../../../lib/providerCharts";
import type {
  CursorModelActivity,
  LocalEffortCost,
  LocalModelCost,
  LocalTokenBreakdown,
  ProviderChartData,
  ProviderUsageSnapshot,
  SettingsSnapshot,
} from "../../../../../types/bridge";
import type { useLocale } from "../../../../../hooks/useLocale";
import { CreditsHistoryChart } from "./CreditsHistoryChart";
import { UsageBreakdownChart } from "./UsageBreakdownChart";
import { QuotaHistoryChart } from "./QuotaHistoryChart";

type T = ReturnType<typeof useLocale>["t"];

interface Props {
  providerId: string;
  accountEmail: string | null;
  providerSnapshot?: ProviderUsageSnapshot;
  t: T;
}

type TabKey = "limits" | "credits" | "usage";

// Provider tabs intentionally remount their chart section. Retain the last
// successful payload for the lifetime of the WebView so returning to a tab is
// instant, then refresh it in the background. The Rust side remains the source
// of truth and maintains its own longer-lived disk cache.
const chartDataCache = new Map<string, ProviderChartData>();

function chartDataCacheKey(providerId: string, accountEmail: string | null, usageWindowsKey = ""): string {
  return `${providerId.toLowerCase()}:${accountEmail?.trim().toLowerCase() ?? ""}:${usageWindowsKey}`;
}

function formatWindowStart(value: string): string {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "current reset period";
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  return new Intl.DateTimeFormat(undefined, sameDay
    ? { hour: "numeric", minute: "2-digit" }
    : { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }
  ).format(date);
}

function formatTokens(value: number | null): string {
  if (value == null) return "—";
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

function TokenMix({ breakdown }: { breakdown: LocalTokenBreakdown }) {
  const cachedTokens = breakdown.cacheReadTokens + breakdown.cacheWriteTokens;
  const cacheShare = breakdown.processedTokens > 0
    ? (cachedTokens / breakdown.processedTokens) * 100
    : 0;
  const items = [
    ["Fresh input", breakdown.freshInputTokens],
    ["Output", breakdown.outputTokens],
    ["Cache read", breakdown.cacheReadTokens],
    ["Cache write", breakdown.cacheWriteTokens],
  ] as const;
  return (
    <div className="usage-token-mix" aria-label="Last 7 days token breakdown">
      <div className="usage-token-mix__header">
        <span className="usage-token-mix__title">Token mix · 7 days</span>
        <span className="usage-token-mix__cache-share">
          {cacheShare.toFixed(1)}% cache traffic
        </span>
      </div>
      <div className="usage-token-mix__items">
        {items.map(([label, value]) => (
          <span className="usage-token-mix__item" key={label}>
            <small>{label}</small>
            <strong>{formatTokens(value)}</strong>
          </span>
        ))}
      </div>
    </div>
  );
}

function formatUsd(value: number): string {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
  }).format(value);
}

function ModelBreakdown({ models }: { models: LocalModelCost[] }) {
  const priced = models.reduce((sum, model) => sum + (model.cost ?? 0), 0);
  const hasUnpriced = models.some((model) => model.cost == null);
  return (
    <div className="usage-model-costs" aria-label="Cost by model over 30 days">
      <div className="usage-model-costs__header">
        <span className="usage-model-costs__title">Cost by model · 30 days</span>
        <span className="usage-model-costs__total">{formatUsd(priced)}</span>
      </div>
      <ul className="usage-model-costs__rows">
        {models.map((model) => (
          <li className="usage-model-costs__row" key={model.model}>
            <span className="usage-model-costs__name" title={model.model}>
              {model.model}
            </span>
            <span className="usage-model-costs__tokens">{formatTokens(model.tokens)}</span>
            <span className="usage-model-costs__cost">
              {model.cost == null ? "Not priced" : formatUsd(model.cost)}
            </span>
          </li>
        ))}
      </ul>
      {hasUnpriced && (
        <p className="usage-model-costs__note">
          Not priced · tokens counted, but no public rate is available for this model.
        </p>
      )}
    </div>
  );
}

const EFFORT_LABELS: Record<string, string> = {
  xhigh: "Extra high",
  high: "High",
  medium: "Medium",
  low: "Low",
  unknown: "Unspecified",
};

function effortLabel(effort: string): string {
  return EFFORT_LABELS[effort] ?? effort;
}

function cursorModelLabel(model: string): string {
  // Cursor records automatic model selection as "default".
  return model === "default" ? "Auto" : model;
}

function CursorActivity({ rows }: { rows: CursorModelActivity[] }) {
  const total = rows.reduce((sum, row) => sum + row.contributions, 0);
  return (
    <div className="cursor-activity" aria-label="Cursor activity by model over 30 days">
      <div className="cursor-activity__header">
        <span className="cursor-activity__title">Cursor activity by model · 30 days</span>
        <span className="cursor-activity__total">{formatTokens(total)} edits</span>
      </div>
      <ul className="cursor-activity__rows">
        {rows.map((row) => {
          const share = total > 0 ? (row.contributions / total) * 100 : 0;
          return (
            <li className="cursor-activity__row" key={row.model}>
              <div className="cursor-activity__row-top">
                <span className="cursor-activity__name" title={row.model}>
                  {cursorModelLabel(row.model)}
                </span>
                <span className="cursor-activity__share">{Math.round(share)}%</span>
              </div>
              <div className="cursor-activity__track" aria-hidden="true">
                <span style={{ width: `${share}%` }} />
              </div>
              <div className="cursor-activity__detail">
                {formatTokens(row.contributions)} edits · {formatTokens(row.requests)} requests
              </div>
            </li>
          );
        })}
      </ul>
      <p className="cursor-activity__note">
        AI code tracked by Cursor Composer, grouped by model. This is activity, not tokens or
        spend (Cursor does not log either locally).
      </p>
    </div>
  );
}

function EffortBreakdown({ efforts }: { efforts: LocalEffortCost[] }) {
  const priced = efforts.reduce((sum, tier) => sum + (tier.cost ?? 0), 0);
  return (
    <div className="usage-model-costs" aria-label="Cost by reasoning effort over 30 days">
      <div className="usage-model-costs__header">
        <span className="usage-model-costs__title">Cost by effort · 30 days</span>
        <span className="usage-model-costs__total">{formatUsd(priced)}</span>
      </div>
      <ul className="usage-model-costs__rows">
        {efforts.map((tier) => (
          <li className="usage-model-costs__row" key={tier.effort}>
            <span className="usage-model-costs__name">{effortLabel(tier.effort)}</span>
            <span className="usage-model-costs__tokens">{formatTokens(tier.tokens)}</span>
            <span className="usage-model-costs__cost">
              {tier.cost == null ? "Not priced" : formatUsd(tier.cost)}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * Charts tabs block for the Settings → Providers detail pane.
 *
 * Port target: cost_history / credits_history / usage_breakdown blocks
 * in `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 *
 * Phase 10: fetches the latest settings snapshot so the animation flag feeds
 * through to each chart component.
 */
export function ChartsSection({ providerId, accountEmail, providerSnapshot, t }: Props) {
  const [data, setData] = useState<ProviderChartData | null>(null);
  const [active, setActive] = useState<TabKey | null>(null);
  const [animations, setAnimations] = useState(true);
  const [loading, setLoading] = useState(true);
  const [enriching, setEnriching] = useState(false);
  const [failed, setFailed] = useState(false);
  const [cursorActivity, setCursorActivity] = useState<CursorModelActivity[] | null>(null);
  const usageWindows = providerLocalUsageWindows(providerSnapshot);
  const usageWindowsKey = usageWindows
    .map((window) => `${window.id}:${window.startsAt}:${window.endsAt}`)
    .join("|");

  useEffect(() => {
    let cancelled = false;
    const cacheKey = chartDataCacheKey(providerId, accountEmail, usageWindowsKey);
    const cached = chartDataCache.get(cacheKey) ?? null;
    setData(cached);
    setActive(null);
    setLoading(cached === null);
    setEnriching(
      cached !== null &&
      !cached.localUsage &&
      ["codex", "claude"].includes(providerId.toLowerCase()),
    );
    setFailed(false);
    if (!providerSupportsChartData(providerId)) {
      setLoading(false);
      return () => {
        cancelled = true;
      };
    }
    getProviderChartData(providerId, accountEmail ?? undefined, usageWindows)
      .then((d) => {
        if (!cancelled) {
          chartDataCache.set(cacheKey, d);
          setData(d);
          setEnriching(
            !d.localUsage && ["codex", "claude"].includes(providerId.toLowerCase()),
          );
          setLoading(false);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setEnriching(false);
          if (cached === null) {
            setData(null);
            setFailed(true);
          }
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [providerId, accountEmail, usageWindowsKey]);

  useEffect(() => {
    let cancelled = false;
    getSettingsSnapshot()
      .then((s: SettingsSnapshot) => {
        if (!cancelled) {
          setAnimations(s.enableAnimations);
        }
      })
      .catch(() => {
        // Keep defaults on failure.
      });
    return () => {
      cancelled = true;
    };
  }, [providerId]);

  useEffect(() => {
    let cancelled = false;
    if (providerId.toLowerCase() !== "cursor") {
      setCursorActivity(null);
      return () => {
        cancelled = true;
      };
    }
    getCursorModelActivity()
      .then((rows) => {
        if (!cancelled) setCursorActivity(rows);
      })
      .catch(() => {
        if (!cancelled) setCursorActivity([]);
      });
    return () => {
      cancelled = true;
    };
  }, [providerId]);

  useEffect(() => {
    if (
      !data ||
      data.localUsage ||
      !["codex", "claude"].includes(providerId.toLowerCase())
    ) {
      if (data?.localUsage || !["codex", "claude"].includes(providerId.toLowerCase())) {
        setEnriching(false);
      }
      return;
    }
    setEnriching(true);
    let cancelled = false;
    let attempts = 0;
    let timer: number | undefined;

    const poll = async () => {
      attempts += 1;
      try {
        const next = await getProviderChartData(
          providerId,
          accountEmail ?? undefined,
          usageWindows,
        );
        if (cancelled) return;
        if (next.localUsage) {
          chartDataCache.set(chartDataCacheKey(providerId, accountEmail, usageWindowsKey), next);
          setData(next);
          setEnriching(false);
          return;
        }
      } catch {
        // Background enrichment is best-effort; keep the quota chart visible.
      }

      if (cancelled) return;
      if (attempts >= 120) {
        setEnriching(false);
        return;
      }

      // Schedule only after the previous read finishes so a slow local scan can
      // never accumulate overlapping work.
      timer = window.setTimeout(() => void poll(), 1_000);
    };

    timer = window.setTimeout(() => void poll(), 1_000);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [data, providerId, accountEmail, usageWindowsKey]);

  if (loading) {
    return (
      <section className="provider-detail-section provider-detail-charts provider-detail-charts--loading">
        <span className="charts-loading__pulse" aria-hidden="true" />
        <div>
          <strong>Reading local history</strong>
          <span>Large transcript libraries can take a moment the first time.</span>
        </div>
      </section>
    );
  }

  if (!data || failed) {
    return (
      <section className="provider-detail-section provider-detail-charts charts-data-empty">
        <strong>History unavailable</strong>
        <span>Ceiling could not read this provider's local history.</span>
      </section>
    );
  }

  const hasCredits = data.creditsHistory.length > 0;
  const hasUsage = data.usageBreakdown.length > 0;
  const hasLimits = data.quotaHistory.length > 0;
  const hasLocalSummary = data.localUsage !== null;
  const hasCursorActivity = (cursorActivity?.length ?? 0) > 0;

  if (!hasCredits && !hasUsage && !hasLimits && !hasLocalSummary && !hasCursorActivity) {
    return (
      <section className="provider-detail-section provider-detail-charts charts-data-empty">
        <strong>History starts here</strong>
        <span>Ceiling will build a 30-day view as it observes this provider.</span>
      </section>
    );
  }

  const available: TabKey[] = [];
  if (hasLimits) available.push("limits");
  if (hasCredits) available.push("credits");
  if (hasUsage) available.push("usage");

  const current: TabKey | null = active && available.includes(active) ? active : available[0] ?? null;
  const emptyMsg = t("DetailChartEmpty");

  const tabLabel = (k: TabKey): string => {
    if (k === "limits") return "Limits";
    if (k === "credits") return t("DetailChartCredits");
    return t("DetailChartUsageBreakdown");
  };

  const usagePeriods = data.localUsage
    ? [
        ...(data.localUsage.currentWindows ?? []).map((window) => ({
          label: window.label,
          tokens: window.tokens,
          detail: `Since ${formatWindowStart(window.startsAt)}`,
          current: true,
        })),
        {
          label: "Last 7 days",
          tokens: data.localUsage.sevenDayTokens,
          detail: "processed tokens",
          current: false,
        },
        {
          label: "Last 30 days",
          tokens: data.localUsage.thirtyDayTokens,
          detail: "processed tokens",
          current: false,
        },
      ]
    : [];

  return (
    <section className="provider-detail-section provider-detail-charts">
      {enriching && (
        <div className="charts-enriching" role="status">
          <span className="charts-loading__pulse" aria-hidden="true" />
          <span>
            <strong>Reading local token history</strong>
            Quota history is ready. Detailed usage will appear automatically.
          </span>
        </div>
      )}
      {cursorActivity && cursorActivity.length > 0 && <CursorActivity rows={cursorActivity} />}
      {data.localUsage && (
        <div
          className="usage-periods"
          data-card-count={usagePeriods.length}
          aria-label="Local usage summary"
        >
          {usagePeriods.map((period) => (
            <div
              className={`usage-period${period.current ? " usage-period--current" : ""}`}
              key={period.label}
            >
              <span>{period.label}</span>
              <strong>{formatTokens(period.tokens)}</strong>
              <small>{period.detail}</small>
            </div>
          ))}
          {data.localUsage.sevenDayTokenBreakdown && (
            <TokenMix breakdown={data.localUsage.sevenDayTokenBreakdown} />
          )}
          <div className="usage-periods__note">
            {data.localUsage.topModel && (
              <span>
                Most used model · <strong>{data.localUsage.topModel}</strong>
              </span>
            )}
            <span>Processed includes fresh input, output, cache reads, and cache writes.</span>
          </div>
          {data.localUsage.modelBreakdown && data.localUsage.modelBreakdown.length > 0 && (
            <ModelBreakdown models={data.localUsage.modelBreakdown} />
          )}
          {data.localUsage.effortBreakdown && data.localUsage.effortBreakdown.length > 0 && (
            <EffortBreakdown efforts={data.localUsage.effortBreakdown} />
          )}
        </div>
      )}
      {available.length > 1 && (
        <div className="provider-detail-charts__tabs" role="tablist">
          {available.map((k) => (
            <button
              key={k}
              type="button"
              role="tab"
              aria-selected={k === current}
              className="provider-detail-charts__tab"
              data-active={k === current ? "true" : "false"}
              onClick={() => setActive(k)}
            >
              {tabLabel(k)}
            </button>
          ))}
        </div>
      )}
      {current && <div className="provider-detail-charts__body" role="tabpanel">
        {current === "limits" && (
          <QuotaHistoryChart
            data={data.quotaHistory}
            providerId={providerId}
            animations={animations}
          />
        )}
        {current === "credits" && (
          <CreditsHistoryChart
            data={data.creditsHistory}
            title={t("DetailChartCredits")}
            ariaLabel={t("DetailChartCredits")}
            providerId={providerId}
            animations={animations}
            emptyMessage={emptyMsg}
          />
        )}
        {current === "usage" && (
          <UsageBreakdownChart
            data={data.usageBreakdown}
            title={t("DetailChartUsageBreakdown")}
            ariaLabel={t("DetailChartUsageBreakdown")}
            animations={animations}
            emptyMessage={emptyMsg}
          />
        )}
      </div>}
    </section>
  );
}
