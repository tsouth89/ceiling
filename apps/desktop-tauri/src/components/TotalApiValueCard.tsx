import { useEffect, useMemo, useState } from "react";
import { getLocalApiValueTotals } from "../lib/tauri";
import type { LocalApiValueProvider } from "../types/bridge";
import { getProviderIcon } from "./providers/providerIcons";
import {
  buildApiValueCard,
  ringSegments,
  type ApiValueMetric,
  type ApiValuePeriodKey,
} from "../lib/apiValueCard";

const PERIODS: { key: ApiValuePeriodKey; label: string }[] = [
  { key: "today", label: "Today" },
  { key: "yesterday", label: "Yesterday" },
  { key: "thirtyDays", label: "30 days" },
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
  const [period, setPeriod] = useState<ApiValuePeriodKey>("today");
  const [metric, setMetric] = useState<ApiValueMetric>("apiValue");

  useEffect(() => {
    let live = true;
    getLocalApiValueTotals()
      .then((rows) => live && setProviders(rows))
      .catch(() => live && setFailed(true));
    return () => {
      live = false;
    };
  }, []);

  const model = useMemo(
    () => (providers ? buildApiValueCard(providers, period, metric) : null),
    [providers, period, metric],
  );

  const formatValue = (value: number) =>
    metric === "apiValue" ? formatUsd(value) : formatTokens(value);

  const periodLabel = PERIODS.find((p) => p.key === period)?.label ?? "";
  const metricLabel = METRICS.find((m) => m.key === metric)?.label ?? "";

  if (failed) {
    return (
      <section className="api-value-card" aria-label="Total API value">
        <p className="api-value-card__status">Local API-value totals are unavailable right now.</p>
      </section>
    );
  }

  if (!model) {
    return (
      <section className="api-value-card" aria-label="Total API value">
        <p className="api-value-card__status">Reading local usage…</p>
      </section>
    );
  }

  const segments = ringSegments(model.slices, CIRCUMFERENCE);
  const coveragePercent =
    model.coverage == null ? null : Math.round(model.coverage * 100);
  const showCoverage = coveragePercent != null && coveragePercent < 100;

  const ariaSummary = model.isEmpty
    ? `No local ${metricLabel} data for ${periodLabel}.`
    : `${metricLabel} for ${periodLabel}: ${formatValue(model.total)} across ${model.slices
        .map((slice) => providerLabel(slice.providerId))
        .join(", ")}.`;

  return (
    <section className="api-value-card" aria-label="Total API value">
      <header className="api-value-card__header">
        <div>
          <h3 className="api-value-card__title">Estimated API value</h3>
          <p className="api-value-card__subtitle">
            API-equivalent estimate from local logs — not subscription spend.
          </p>
        </div>
        <div className="api-value-card__switchers">
          <div className="api-value-card__switch" role="tablist" aria-label="Period">
            {PERIODS.map((p) => (
              <button
                key={p.key}
                type="button"
                role="tab"
                aria-selected={p.key === period}
                data-active={p.key === period}
                className="api-value-card__switch-btn"
                onClick={() => setPeriod(p.key)}
              >
                {p.label}
              </button>
            ))}
          </div>
          <div className="api-value-card__switch" role="tablist" aria-label="Metric">
            {METRICS.map((m) => (
              <button
                key={m.key}
                type="button"
                role="tab"
                aria-selected={m.key === metric}
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

      {model.isEmpty ? (
        <p className="api-value-card__status" role="status">
          No data for {periodLabel}.
        </p>
      ) : (
        <div className="api-value-card__body">
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
              <strong>{formatValue(model.total)}</strong>
              <small>{periodLabel}</small>
            </div>
          </div>

          <ul className="api-value-card__legend">
            {model.slices.map((slice) => (
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
                <span className="api-value-card__legend-value">{formatValue(slice.value)}</span>
              </li>
            ))}
          </ul>
        </div>
      )}

      {!model.isEmpty && (
        <p className="api-value-card__note">
          <span className="api-value-card__estimate-marker" aria-hidden="true">
            ~
          </span>
          Estimated API value.
          {showCoverage && (
            <>
              {" "}
              {coveragePercent}% of tokens priced
              {model.unpricedProviderIds.length > 0 &&
                ` (unpriced models in ${model.unpricedProviderIds
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
