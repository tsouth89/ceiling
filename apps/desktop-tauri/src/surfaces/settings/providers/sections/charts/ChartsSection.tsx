import { useEffect, useState } from "react";
import {
  exportCostCsv,
  getCursorModelActivity,
  getProviderChartData,
  getSettingsSnapshot,
} from "../../../../../lib/tauri";
import {
  providerHasUnavailableResetBoundary,
  providerLocalUsageWindows,
  providerSupportsChartData,
} from "../../../../../lib/providerCharts";
import type {
  CursorModelActivity,
  LocalEffortCost,
  LocalModelCost,
  LocalPlanUsage,
  LocalProjectCost,
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

function chartDataCacheKey(
  providerId: string,
  accountEmail: string | null,
  sourceLabel: string | undefined,
  usageWindowsKey = "",
): string {
  return `${providerId.toLowerCase()}:${accountEmail?.trim().toLowerCase() ?? ""}:${sourceLabel?.trim().toLowerCase() ?? ""}:${usageWindowsKey}`;
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

function formatWindowReset(value: string): string {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "at the provider-reported boundary";
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

/** Backend bucket for records whose rollout never declared a plan. */
const UNATTRIBUTED_PLAN = "unattributed";

/**
 * Disclose that these totals are not account-scoped.
 *
 * Local logs record the plan but never the account, for any provider. So more
 * than one plan proves the totals cover more than one plan - NOT that they came
 * from someone else's account. One account changing plans looks exactly the
 * same. The copy therefore states the limitation and what was observed, and
 * stops short of naming accounts.
 *
 * `unattributed` is a bucket, not a plan, so it can never trigger this on its
 * own: one plan plus some unlabeled records is still one plan.
 */
function MultiPlanNotice({ plans }: { plans?: LocalPlanUsage[] | null }) {
  if (!plans || plans.length === 0) return null;
  const named = plans.filter((plan) => plan.plan !== UNATTRIBUTED_PLAN);
  if (named.length < 2) return null;

  const total = plans.reduce((sum, plan) => sum + plan.tokens, 0);
  if (total <= 0) return null;
  const share = (tokens: number) => {
    const percent = (tokens / total) * 100;
    return percent < 1 ? "<1%" : `${Math.round(percent)}%`;
  };
  const unlabeled = plans.find((plan) => plan.plan === UNATTRIBUTED_PLAN);

  return (
    <p className="usage-periods__note usage-periods__note--warn" role="note">
      <span>
        These totals are <strong>not account-scoped</strong>. Local logs record
        the plan but not the account. Over the last 30 days this machine shows{" "}
        {named.length} plans ({named.map((p) => `${p.plan} ${share(p.tokens)}`).join(", ")}
        {unlabeled ? `, unlabeled ${share(unlabeled.tokens)}` : ""}).
      </span>
    </p>
  );
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
            <div className="usage-model-costs__main">
              <span className="usage-model-costs__name" title={model.model}>
                {model.model}
              </span>
              <span className="usage-model-costs__tokens">{formatTokens(model.tokens)}</span>
              <span className="usage-model-costs__cost">
                {model.cost == null ? "Not priced" : formatUsd(model.cost)}
              </span>
            </div>
            <div className="usage-model-costs__metrics">
              {model.cacheReadPercent != null && (
                /* Its own period, because the "· 30 days" header is rows away
                   and the token-mix card shows a different window above. */
                <span>{model.cacheReadPercent.toFixed(0)}% cache read · 30 days</span>
              )}
              {model.costPerCall != null && (
                <span>{formatUsd(model.costPerCall)}/call</span>
              )}
              {model.outputTokensPerCall != null && (
                <span>{formatTokens(Math.round(model.outputTokensPerCall))} out/call</span>
              )}
            </div>
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

function projectLabel(project: string): string {
  return project === "unknown" ? "Unknown project" : project;
}

function ExportCsvButton({ providerId }: { providerId: string }) {
  const [status, setStatus] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const onExport = async () => {
    setBusy(true);
    setStatus(null);
    try {
      const path = await exportCostCsv(providerId);
      setStatus({ tone: "ok", text: `Saved to ${path}` });
    } catch (error) {
      setStatus({
        tone: "err",
        text: typeof error === "string" ? error : "Export failed.",
      });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="usage-export">
      <button
        type="button"
        className="usage-export__btn"
        onClick={onExport}
        disabled={busy}
      >
        {busy ? "Exporting…" : "Export CSV"}
      </button>
      {status && (
        <span className={`usage-export__status usage-export__status--${status.tone}`}>
          {status.text}
        </span>
      )}
    </div>
  );
}

const PROJECT_COLLAPSED_COUNT = 8;

function ProjectBreakdown({ projects }: { projects: LocalProjectCost[] }) {
  const [expanded, setExpanded] = useState(false);
  const priced = projects.reduce((sum, project) => sum + (project.cost ?? 0), 0);
  const hasUnpriced = projects.some((project) => project.cost == null);
  const hasMore = projects.length > PROJECT_COLLAPSED_COUNT;
  const visible = expanded ? projects : projects.slice(0, PROJECT_COLLAPSED_COUNT);
  return (
    <div className="usage-model-costs" aria-label="Cost by project over 30 days">
      <div className="usage-model-costs__header">
        <span className="usage-model-costs__title">Cost by project · 30 days</span>
        <span className="usage-model-costs__total">{formatUsd(priced)}</span>
      </div>
      <ul className="usage-model-costs__rows">
        {visible.map((project) => (
          <li className="usage-model-costs__row" key={project.project}>
            <span className="usage-model-costs__name" title={project.project}>
              {projectLabel(project.project)}
            </span>
            <span className="usage-model-costs__tokens">{formatTokens(project.tokens)}</span>
            <span className="usage-model-costs__cost">
              {project.cost == null ? "Not priced" : formatUsd(project.cost)}
            </span>
          </li>
        ))}
      </ul>
      {hasMore && (
        <button
          type="button"
          className="usage-model-costs__more"
          aria-expanded={expanded}
          onClick={() => setExpanded((open) => !open)}
        >
          {expanded ? "Show fewer" : `Show all ${projects.length} projects`}
        </button>
      )}
      {hasUnpriced && (
        <p className="usage-model-costs__note">
          Not priced · tokens counted, but the models used have no public rate.
        </p>
      )}
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
  const sourceLabel = providerSnapshot?.sourceLabel;
  const resetBoundaryUnavailable = providerHasUnavailableResetBoundary(providerSnapshot);
  const usageWindowsKey = usageWindows
    .map((window) => `${window.id}:${window.startsAt}:${window.endsAt}`)
    .join("|");

  useEffect(() => {
    let cancelled = false;
    const cacheKey = chartDataCacheKey(providerId, accountEmail, sourceLabel, usageWindowsKey);
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
    getProviderChartData(providerId, accountEmail ?? undefined, usageWindows, sourceLabel)
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
  }, [providerId, accountEmail, sourceLabel, usageWindowsKey]);

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
          sourceLabel,
        );
        if (cancelled) return;
        if (next.localUsage) {
          chartDataCache.set(chartDataCacheKey(providerId, accountEmail, sourceLabel, usageWindowsKey), next);
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
  }, [data, providerId, accountEmail, sourceLabel, usageWindowsKey]);

  // Cursor activity is independent of chart history and only belongs to the
  // Cursor provider. Guard on the current provider so a stale fetch from a
  // previous selection can't flash in another provider's view.
  const isCursor = providerId.toLowerCase() === "cursor";
  const cursorRows = isCursor ? cursorActivity ?? [] : [];
  const hasCursorActivity = cursorRows.length > 0;

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
    // Chart history is unavailable, but Cursor's local activity may still be
    // readable — show it rather than a bare error.
    if (hasCursorActivity) {
      return (
        <section className="provider-detail-section provider-detail-charts">
          <CursorActivity rows={cursorRows} />
        </section>
      );
    }
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

  if (!hasCredits && !hasUsage && !hasLimits && !hasLocalSummary && !hasCursorActivity
    && !resetBoundaryUnavailable) {
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
          cost: window.cost ?? null,
          detail: `Since ${formatWindowStart(window.startsAt)}`,
          detailSecondary: `Resets ${formatWindowReset(window.endsAt)}`,
          current: true,
        })),
        {
          label: "Last 7 days",
          tokens: data.localUsage.sevenDayTokens,
          cost: data.localUsage.sevenDayCost ?? null,
          detail: "Processed tokens · calendar",
          detailSecondary: null as string | null,
          current: false,
        },
        {
          label: "Last 30 days",
          tokens: data.localUsage.thirtyDayTokens,
          cost: data.localUsage.thirtyDayCost ?? null,
          detail: "Processed tokens · calendar",
          detailSecondary: null as string | null,
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
      {hasCursorActivity && <CursorActivity rows={cursorRows} />}
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
              {period.cost != null && (
                <span className="usage-period__money">{formatUsd(period.cost)}</span>
              )}
              <small>
                {period.detail}
                {period.detailSecondary && (
                  <>
                    <br />
                    {period.detailSecondary}
                  </>
                )}
              </small>
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
          {/* Local logs carry no account identity, so these totals cover every
              account that has used this machine. Disclose that rather than let
              the figures read as belonging to the signed-in account. */}
          <MultiPlanNotice plans={data.localUsage.planBreakdown} />
          {data.localUsage.modelBreakdown && data.localUsage.modelBreakdown.length > 0 && (
            <ModelBreakdown models={data.localUsage.modelBreakdown} />
          )}
          {data.localUsage.effortBreakdown && data.localUsage.effortBreakdown.length > 0 && (
            <EffortBreakdown efforts={data.localUsage.effortBreakdown} />
          )}
          {data.localUsage.projectBreakdown && data.localUsage.projectBreakdown.length > 0 && (
            <ProjectBreakdown projects={data.localUsage.projectBreakdown} />
          )}
          <ExportCsvButton providerId={providerId} />
        </div>
      )}
      {resetBoundaryUnavailable && (
        <div className="usage-periods__note" role="status">
          Reset boundary unavailable. Ceiling will not substitute a rolling period.
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
