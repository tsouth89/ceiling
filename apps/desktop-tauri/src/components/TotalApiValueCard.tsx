import { useEffect, useMemo, useState } from "react";
import { getLocalApiValueTotals } from "../lib/tauri";
import type { LocalApiValueDay, LocalApiValueProvider } from "../types/bridge";
import { getProviderIcon } from "./providers/providerIcons";
import {
  buildApiValueCard,
  formatPeriodChange,
  periodFromDailySeries,
  ringSegments,
  type ApiValueMetric,
  type ApiValuePeriodKey,
} from "../lib/apiValueCard";

const PERIODS: { key: ApiValuePeriodKey; label: string }[] = [
  { key: "today", label: "Today" },
  { key: "yesterday", label: "Yesterday" },
  { key: "thirtyDays", label: "30 days" },
  { key: "custom", label: "Custom" },
];

const METRICS: { key: ApiValueMetric; label: string }[] = [
  { key: "apiValue", label: "API value" },
  { key: "tokens", label: "Tokens" },
];

const RING_RADIUS = 52;
const RING_THICKNESS = 14;
const CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;

function formatUsd(value: number): string {
  return new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" }).format(value);
}

function formatTokens(value: number): string {
  return new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 }).format(value);
}

function providerLabel(providerId: string): string {
  return providerId.charAt(0).toUpperCase() + providerId.slice(1);
}

function providerColor(providerId: string): string {
  return getProviderIcon(providerId).brandColor;
}

/** Local calendar date as YYYY-MM-DD. */
function formatLocalDate(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

function defaultCustomRange(): { since: string; until: string } {
  const until = new Date();
  const since = new Date();
  since.setDate(since.getDate() - 6); // inclusive 7 local days ending today
  return { since: formatLocalDate(since), until: formatLocalDate(until) };
}

/** Compact range label without fancy dashes (plain hyphen). */
function shortRangeLabel(since: string, until: string): string {
  if (since === until) return since;
  const parse = (iso: string) => {
    const [y, m, d] = iso.split("-").map(Number);
    if (!y || !m || !d) return null;
    return new Date(y, m - 1, d);
  };
  const a = parse(since);
  const b = parse(until);
  if (!a || !b) return `${since} - ${until}`;
  const fmt = (date: Date) =>
    date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  if (a.getFullYear() === b.getFullYear()) {
    return `${fmt(a)} - ${fmt(b)}`;
  }
  return `${since} - ${until}`;
}

function tauriErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    const message = (err as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  return String(err ?? "");
}

type TrendDay = {
  date: string;
  label: string;
  value: number;
  height: number;
};

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function dayLabel(date: string): string {
  const parsed = new Date(`${date}T12:00:00`);
  if (Number.isNaN(parsed.getTime())) return date.slice(5);
  return WEEKDAYS[parsed.getDay()];
}

/** Sum each provider's day series into one per-day trend. */
function buildTrend(
  providers: LocalApiValueProvider[],
  period: ApiValuePeriodKey,
  since: string,
  until: string,
): TrendDay[] {
  const totals = new Map<string, number>();
  for (const provider of providers) {
    const series: LocalApiValueDay[] =
      period === "custom"
        ? (provider.dailySeries?.length
            ? provider.dailySeries
            : provider.lastSevenDays ?? [])
        : (provider.lastSevenDays ?? []);
    for (const day of series) {
      if (period === "custom" && (day.date < since || day.date > until)) continue;
      totals.set(day.date, (totals.get(day.date) ?? 0) + day.apiValueUsd);
    }
  }
  const dates = [...totals.keys()].sort();
  if (dates.length === 0) return [];
  const peak = Math.max(...dates.map((date) => totals.get(date) ?? 0));
  return dates.map((date) => {
    const value = totals.get(date) ?? 0;
    return {
      date,
      label: dayLabel(date),
      value,
      height: peak > 0 ? Math.max(3, (value / peak) * 100) : 3,
    };
  });
}

/**
 * Aggregate "estimated API value" across providers, from local logs.
 *
 * Token-derived dollars are an API-equivalent estimate, never a bill. Providers
 * with no data this period are omitted; an entirely empty period shows "No
 * data". Pricing coverage is surfaced whenever any tokens are unpriced.
 */
export function TotalApiValueCard() {
  const [providers, setProviders] = useState<LocalApiValueProvider[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [period, setPeriod] = useState<ApiValuePeriodKey>("today");
  const [metric, setMetric] = useState<ApiValueMetric>("apiValue");
  const [customSince, setCustomSince] = useState(() => defaultCustomRange().since);
  const [customUntil, setCustomUntil] = useState(() => defaultCustomRange().until);
  const [rangeError, setRangeError] = useState<string | null>(null);

  const customRange = useMemo(
    () =>
      period === "custom" && customSince && customUntil
        ? { since: customSince, until: customUntil }
        : null,
    [period, customSince, customUntil],
  );

  // One scan answers every built-in period, so this depends on the custom range
  // alone: switching Today / Yesterday / 30d re-reads what is already in state
  // instead of re-running a multi-gigabyte transcript scan per click.
  useEffect(() => {
    let live = true;
    setFailed(false);
    setRangeError(null);
    setLoading(true);

    getLocalApiValueTotals(customRange ?? undefined)
      .then((rows) => {
        if (!live) return;
        // If the backend forgot custom but daily series covers the range, fill
        // it here so the ring never sits empty after a successful scan.
        if (customRange) {
          setProviders(
            rows.map((row) => {
              if (row.custom?.hasData) return row;
              const series = row.dailySeries?.length
                ? row.dailySeries
                : row.lastSevenDays;
              const synthesized = periodFromDailySeries(
                series,
                customRange.since,
                customRange.until,
              );
              return synthesized.hasData ? { ...row, custom: synthesized } : row;
            }),
          );
        } else {
          setProviders(rows);
        }
      })
      .catch((err: unknown) => {
        if (!live) return;
        const message = tauriErrorMessage(err);
        if (customRange && message) {
          setRangeError(message);
          return;
        }
        setFailed(true);
      })
      .finally(() => {
        if (live) setLoading(false);
      });
    return () => {
      live = false;
    };
  }, [customRange]);

  const model = useMemo(
    () =>
      providers
        ? buildApiValueCard(providers, period, metric, customRange)
        : null,
    [providers, period, metric, customRange],
  );

  const formatValue = (value: number) =>
    metric === "apiValue" ? formatUsd(value) : formatTokens(value);

  const periodLabel =
    period === "custom"
      ? shortRangeLabel(customSince, customUntil)
      : (PERIODS.find((p) => p.key === period)?.label ?? "");
  const metricLabel = METRICS.find((m) => m.key === metric)?.label ?? "";
  const todayIso = formatLocalDate(new Date());
  const ringCaption = period === "custom" ? "Custom" : periodLabel;

  if (failed) {
    return (
      <section className="api-value-card" aria-label="Total API value">
        <p className="api-value-card__status">Local API-value totals are unavailable right now.</p>
      </section>
    );
  }

  if (!model && loading) {
    return (
      <section className="api-value-card" aria-label="Total API value">
        <p className="api-value-card__status">Reading local usage…</p>
      </section>
    );
  }

  const safeModel = model ?? buildApiValueCard([], period, metric, customRange);
  const segments = ringSegments(safeModel.slices, CIRCUMFERENCE);
  const coveragePercent =
    safeModel.coverage == null ? null : Math.round(safeModel.coverage * 100);
  const showCoverage = safeModel.coverage != null && safeModel.coverage < 1;
  const periodChangeLabel =
    safeModel.periodChange && metric === "apiValue"
      ? formatPeriodChange(safeModel.periodChange)
      : null;

  const trend = buildTrend(
    providers ?? [],
    period,
    customSince,
    customUntil,
  );
  const trendTotal = trend.reduce((sum, day) => sum + day.value, 0);
  const trendTitle =
    period === "custom" ? shortRangeLabel(customSince, customUntil) : "Last 7 days";

  const ariaSummary = loading
    ? `Loading ${metricLabel} for ${periodLabel}.`
    : safeModel.isEmpty
      ? `No local ${metricLabel} data for ${periodLabel}.`
      : `${metricLabel} for ${periodLabel}: ${formatValue(safeModel.total)} across ${safeModel.slices
          .map((slice) => providerLabel(slice.providerId))
          .join(", ")}.`;

  return (
    <section className="api-value-card" aria-label="Total API value">
      <header className="api-value-card__header">
        <div>
          <h3 className="api-value-card__title">Estimated API value</h3>
          <p className="api-value-card__subtitle">
            API-equivalent estimate from local logs - not subscription spend.
          </p>
        </div>
        <div className="api-value-card__switchers">
          <div className="api-value-card__switch" role="group" aria-label="Period">
            {PERIODS.map((p) => (
              <button
                key={p.key}
                type="button"
                aria-pressed={p.key === period}
                data-active={p.key === period}
                className="api-value-card__switch-btn"
                onClick={() => setPeriod(p.key)}
              >
                {p.label}
              </button>
            ))}
          </div>
          <div className="api-value-card__switch" role="group" aria-label="Metric">
            {METRICS.map((m) => (
              <button
                key={m.key}
                type="button"
                aria-pressed={m.key === metric}
                data-active={m.key === metric}
                className="api-value-card__switch-btn"
                onClick={() => setMetric(m.key)}
              >
                {m.label}
              </button>
            ))}
          </div>
        </div>
      </header>

      {period === "custom" && (
        <div className="api-value-card__custom-range" role="group" aria-label="Custom date range">
          <label className="api-value-card__date-field">
            <span>From</span>
            <input
              type="date"
              value={customSince}
              max={customUntil || todayIso}
              onChange={(event) => setCustomSince(event.target.value)}
            />
          </label>
          <span className="api-value-card__date-sep" aria-hidden>
            to
          </span>
          <label className="api-value-card__date-field">
            <span>To</span>
            <input
              type="date"
              value={customUntil}
              min={customSince || undefined}
              max={todayIso}
              onChange={(event) => setCustomUntil(event.target.value)}
            />
          </label>
          <span className="api-value-card__range-pill" title={periodLabel}>
            {periodLabel}
          </span>
          {rangeError && (
            <p className="api-value-card__range-error" role="alert">
              {rangeError}
            </p>
          )}
        </div>
      )}

      {loading ? (
        <p className="api-value-card__status" role="status">
          Reading local usage…
        </p>
      ) : (
        <div className="api-value-card__body">
          <div className="api-value-card__ring-wrap">
            <div className="api-value-card__ring" role="img" aria-label={ariaSummary}>
              <svg viewBox="0 0 120 120" className="api-value-card__ring-svg">
                <circle
                  cx="60"
                  cy="60"
                  r={RING_RADIUS}
                  fill="none"
                  stroke="var(--ceiling-glass-border)"
                  strokeWidth={RING_THICKNESS}
                  opacity={0.35}
                />
                <g transform="rotate(-90 60 60)">
                  {segments.map((segment) => (
                    <circle
                      key={segment.providerId}
                      cx="60"
                      cy="60"
                      r={RING_RADIUS}
                      fill="none"
                      stroke={providerColor(segment.providerId)}
                      strokeWidth={RING_THICKNESS}
                      strokeDasharray={`${segment.dash} ${CIRCUMFERENCE - segment.dash}`}
                      strokeDashoffset={segment.offset}
                      strokeLinecap="butt"
                    />
                  ))}
                </g>
              </svg>
              <div className="api-value-card__ring-center">
                <strong>
                  {safeModel.isEmpty || rangeError ? formatValue(0) : formatValue(safeModel.total)}
                </strong>
                <small>{ringCaption}</small>
              </div>
            </div>
            <span className="api-value-card__period-change">
              {rangeError
                ? "Adjust the dates to load a range."
                : safeModel.isEmpty
                  ? `No data for ${periodLabel}`
                  : (periodChangeLabel ?? "")}
            </span>
          </div>

          {!safeModel.isEmpty && !rangeError && (
            <ul className="api-value-card__legend">
              {safeModel.slices.map((slice) => (
                <li className="api-value-card__legend-row" key={slice.providerId}>
                  <span
                    className="api-value-card__legend-dot"
                    style={{ background: providerColor(slice.providerId) }}
                    aria-hidden="true"
                  />
                  <span className="api-value-card__legend-name">
                    {providerLabel(slice.providerId)}
                  </span>
                  <span className="api-value-card__legend-share">
                    {Math.round(slice.share * 100)}%
                  </span>
                  <span className="api-value-card__legend-value">
                    {formatValue(slice.value)}
                  </span>
                </li>
              ))}
            </ul>
          )}

          {trend.length > 0 && !rangeError && (
            <div className="api-value-card__trend">
              <div className="api-value-card__trend-head">
                <span>{trendTitle}</span>
                <strong>{formatUsd(trendTotal)}</strong>
              </div>
              <div className="api-value-card__trend-bars">
                {trend.map((day, index) => (
                  <span
                    key={day.date}
                    className="api-value-card__trend-bar"
                    data-today={index === trend.length - 1}
                    style={{ height: `${day.height}%` }}
                    title={`${day.date}: ${formatUsd(day.value)}`}
                  />
                ))}
              </div>
              <div className="api-value-card__trend-days">
                {trend.map((day) => (
                  <span key={day.date}>{day.label}</span>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {!loading && !safeModel.isEmpty && !rangeError && (
        <p className="api-value-card__note">
          <span className="api-value-card__estimate-marker" aria-hidden="true">
            ~
          </span>
          Estimated API value.
          {showCoverage && (
            <>
              {" "}
              {coveragePercent}% of tokens priced
              {safeModel.unpricedProviderIds.length > 0 &&
                ` (unpriced models in ${safeModel.unpricedProviderIds
                  .map(providerLabel)
                  .join(", ")})`}
              .
            </>
          )}
        </p>
      )}
    </section>
  );
}
