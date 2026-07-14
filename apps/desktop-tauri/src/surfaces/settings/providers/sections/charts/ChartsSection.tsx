import { useEffect, useState } from "react";
import { getProviderChartData, getSettingsSnapshot } from "../../../../../lib/tauri";
import { providerSupportsChartData } from "../../../../../lib/providerCharts";
import type {
  LocalTokenBreakdown,
  ProviderChartData,
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
  t: T;
}

type TabKey = "limits" | "credits" | "usage";

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

/**
 * Charts tabs block for the Settings → Providers detail pane.
 *
 * Port target: cost_history / credits_history / usage_breakdown blocks
 * in `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 *
 * Phase 10: fetches the latest settings snapshot so the animation flag feeds
 * through to each chart component.
 */
export function ChartsSection({ providerId, accountEmail, t }: Props) {
  const [data, setData] = useState<ProviderChartData | null>(null);
  const [active, setActive] = useState<TabKey | null>(null);
  const [animations, setAnimations] = useState(true);
  const [loading, setLoading] = useState(true);
  const [enriching, setEnriching] = useState(false);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setData(null);
    setActive(null);
    setLoading(true);
    setEnriching(false);
    setFailed(false);
    if (!providerSupportsChartData(providerId)) {
      setLoading(false);
      return () => {
        cancelled = true;
      };
    }
    getProviderChartData(providerId, accountEmail ?? undefined)
      .then((d) => {
        if (!cancelled) {
          setData(d);
          setEnriching(
            !d.localUsage && ["codex", "claude"].includes(providerId.toLowerCase()),
          );
          setLoading(false);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setData(null);
          setEnriching(false);
          setFailed(true);
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [providerId, accountEmail]);

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
    let attempts = 0;
    const timer = window.setInterval(() => {
      attempts += 1;
      getProviderChartData(providerId, accountEmail ?? undefined)
        .then((next) => {
          if (next.localUsage) {
            setData(next);
            setEnriching(false);
            window.clearInterval(timer);
          }
        })
        .catch(() => {
          // Background enrichment is best-effort; keep the quota chart visible.
        });
      if (attempts >= 120) {
        setEnriching(false);
        window.clearInterval(timer);
      }
    }, 1_000);
    return () => window.clearInterval(timer);
  }, [data, providerId, accountEmail]);

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

  if (!hasCredits && !hasUsage && !hasLimits && !hasLocalSummary) {
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
      {data.localUsage && (
        <div className="usage-periods" aria-label="Local usage summary">
          {[
            {
              label: "Last session",
              tokens: data.localUsage.lastSessionTokens,
            },
            {
              label: "Last 7 days",
              tokens: data.localUsage.sevenDayTokens,
            },
            {
              label: "Last 30 days",
              tokens: data.localUsage.thirtyDayTokens,
            },
          ].map((period) => (
            <div className="usage-period" key={period.label}>
              <span>{period.label}</span>
              <strong>{formatTokens(period.tokens)}</strong>
              <small>processed tokens</small>
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
