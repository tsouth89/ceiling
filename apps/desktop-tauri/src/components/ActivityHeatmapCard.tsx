import { useEffect, useMemo, useRef, useState } from "react";
import { getLocalActivityHeatmap } from "../lib/tauri";
import { useLocalScanRefresh } from "../hooks/useLocalScanRefresh";
import type { ActivityHeatmap } from "../types/bridge";
import { getProviderIcon } from "./providers/providerIcons";
import {
  chartTooltipPosition,
  type ChartTooltipPosition,
} from "./charts/chartTooltip";
import {
  WEEKDAY_LABELS,
  buildCalendar,
  buildWeekHourGrid,
  formatHourLabel,
  peakHour,
  peakWeekday,
  selectProviders,
  totalValue,
  type ActivityMetric,
} from "../lib/activityHeatmap";

const METRICS: { key: ActivityMetric; label: string }[] = [
  { key: "apiValue", label: "API value" },
  { key: "tokens", label: "Tokens" },
];

/** Hour ticks every six hours keeps 24 columns legible at panel width. */
const HOUR_TICKS = [0, 6, 12, 18];

function formatUsd(value: number): string {
  return new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" }).format(value);
}

function formatTokens(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

function formatMetric(value: number, metric: ActivityMetric): string {
  return metric === "apiValue" ? formatUsd(value) : `${formatTokens(value)} tokens`;
}

function providerLabel(providerId: string): string {
  return providerId.charAt(0).toUpperCase() + providerId.slice(1);
}

/** "Aug 15" for cell labels; the year is already implied by a 30-day window. */
function shortDate(date: string): string {
  const [year, month, day] = date.split("-").map(Number);
  if (!year || !month || !day) return date;
  return new Date(year, month - 1, day).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

function tauriErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return String(err ?? "");
}

type Tooltip = ChartTooltipPosition & { text: string };

/**
 * When the machine is actually busy, from local transcript timestamps (SBS-277).
 *
 * Two views over the same data: a 30-day calendar strip for "which days", and a
 * weekday-by-hour grid for "which hours". Both use one sequential ramp, and
 * every cell carries its exact figure in text so the reading never depends on
 * telling two shades apart.
 */
export function ActivityHeatmapCard() {
  const [heatmap, setHeatmap] = useState<ActivityHeatmap | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [metric, setMetric] = useState<ActivityMetric>("apiValue");
  const [excluded, setExcluded] = useState<string[]>([]);
  const [tooltip, setTooltip] = useState<Tooltip | null>(null);
  const hostRef = useRef<HTMLDivElement | null>(null);

  // Ticks when a rescan started behind a stale answer has finished.
  const refreshes = useLocalScanRefresh("activity-heatmap");

  useEffect(() => {
    let live = true;
    setLoading(true);
    setError(null);
    getLocalActivityHeatmap()
      .then((data) => {
        if (live) setHeatmap(data);
      })
      .catch((err: unknown) => {
        if (live) setError(tauriErrorMessage(err) || "Unable to read local activity.");
      })
      .finally(() => {
        if (live) setLoading(false);
      });
    return () => {
      live = false;
    };
  }, [refreshes]);

  const providerIds = heatmap?.providerIds ?? [];
  // Chips toggle providers off, so a provider that appears after a rescan is
  // visible by default rather than silently filtered out.
  const visibleProviders = useMemo(
    () => providerIds.filter((id) => !excluded.includes(id)),
    [providerIds, excluded],
  );

  const rows = useMemo(
    () => selectProviders(heatmap?.hours ?? [], visibleProviders),
    [heatmap, visibleProviders],
  );

  const calendar = useMemo(
    () => buildCalendar(heatmap?.days ?? [], rows, metric),
    [heatmap, rows, metric],
  );
  const grid = useMemo(() => buildWeekHourGrid(rows, metric), [rows, metric]);
  const busiestHour = useMemo(() => peakHour(rows, metric), [rows, metric]);
  const busiestWeekday = useMemo(() => peakWeekday(rows, metric), [rows, metric]);
  const total = useMemo(() => totalValue(rows, metric), [rows, metric]);

  const showTooltip = (event: React.MouseEvent<HTMLElement>, text: string) => {
    const host = hostRef.current;
    if (!host) return;
    setTooltip({ text, ...chartTooltipPosition(event.clientX, event.clientY, host.getBoundingClientRect()) });
  };
  const hideTooltip = () => setTooltip(null);

  if (loading) {
    return (
      <section className="activity-card" aria-label="Activity heatmap">
        <p className="activity-card__status">Reading local activity…</p>
      </section>
    );
  }

  if (error) {
    return (
      <section className="activity-card" aria-label="Activity heatmap">
        <p className="activity-card__status">{error}</p>
      </section>
    );
  }

  const hasActivity = rows.length > 0;
  // An empty grid because every chip is off is a different thing from an empty
  // grid because the machine was idle, and saying the wrong one reads as a bug.
  const allProvidersHidden = providerIds.length > 0 && visibleProviders.length === 0;

  return (
    <section className="activity-card" aria-label="Activity heatmap">
      <header className="activity-card__header">
        <div>
          <h3 className="activity-card__title">When you work</h3>
          <p className="activity-card__subtitle">
            Last {calendar.length} days of local activity, {heatmap?.timezoneLabel ?? "local time"}
          </p>
        </div>
        <div className="activity-card__switch" role="group" aria-label="Metric">
          {METRICS.map((option) => (
            <button
              key={option.key}
              type="button"
              className="activity-card__switch-btn"
              data-active={metric === option.key ? "true" : "false"}
              aria-pressed={metric === option.key}
              onClick={() => setMetric(option.key)}
            >
              {option.label}
            </button>
          ))}
        </div>
      </header>

      {providerIds.length > 1 && (
        <div className="activity-card__providers" role="group" aria-label="Providers">
          {providerIds.map((id) => {
            const on = !excluded.includes(id);
            return (
              <button
                key={id}
                type="button"
                className="activity-card__provider-chip"
                data-active={on ? "true" : "false"}
                aria-pressed={on}
                onClick={() =>
                  setExcluded((prev) =>
                    prev.includes(id) ? prev.filter((entry) => entry !== id) : [...prev, id],
                  )
                }
              >
                <span
                  className="activity-card__provider-dot"
                  style={{ background: getProviderIcon(id).brandColor }}
                  aria-hidden
                />
                {providerLabel(id)}
              </button>
            );
          })}
        </div>
      )}

      <div className="activity-card__plots" ref={hostRef}>
        {!hasActivity && (
          <p className="activity-card__status" role="status">
            {allProvidersHidden
              ? "Every provider is hidden. Turn one back on above."
              : "No local activity in this window yet."}
          </p>
        )}

        <div className="activity-card__section">
          <div className="activity-card__section-head">
            <span className="activity-card__section-title">By day</span>
            <span className="activity-card__section-note">
              {formatMetric(total, metric)} total
            </span>
          </div>
          <div
            className="activity-card__days"
            role="img"
            aria-label={`Daily activity for the last ${calendar.length} days`}
          >
            {calendar.map((cell) => {
              const label = `${shortDate(cell.date)}: ${formatMetric(cell.value, metric)}, ${cell.calls} calls`;
              return (
                <div
                  key={cell.date}
                  className="activity-card__cell activity-card__cell--day"
                  data-level={cell.level}
                  title={label}
                  onMouseMove={(event) => showTooltip(event, label)}
                  onMouseLeave={hideTooltip}
                />
              );
            })}
          </div>
          {calendar.length > 0 && (
            <div className="activity-card__days-axis" aria-hidden>
              <span>{shortDate(calendar[0].date)}</span>
              <span>{shortDate(calendar[calendar.length - 1].date)}</span>
            </div>
          )}
        </div>

        <div className="activity-card__section">
          <div className="activity-card__section-head">
            <span className="activity-card__section-title">By hour</span>
            <span className="activity-card__section-note">
              {busiestHour && busiestWeekday
                ? `Busiest ${WEEKDAY_LABELS[busiestWeekday.weekday]}, around ${formatHourLabel(busiestHour.hour)}`
                : "No peak yet"}
            </span>
          </div>
          <div className="activity-card__grid-wrap">
            <div className="activity-card__hour-axis" aria-hidden>
              {HOUR_TICKS.map((hour) => (
                <span key={hour} style={{ gridColumn: `${hour + 1} / span 6` }}>
                  {formatHourLabel(hour)}
                </span>
              ))}
            </div>
            <div
              className="activity-card__grid"
              role="img"
              aria-label="Activity by weekday and hour of day"
            >
              {grid.map((row, weekday) => (
                <div className="activity-card__grid-row" key={weekday}>
                  <span className="activity-card__weekday" aria-hidden>
                    {WEEKDAY_LABELS[weekday]}
                  </span>
                  {row.map((cell) => {
                    const label = `${WEEKDAY_LABELS[weekday]} ${formatHourLabel(cell.hour)}: ${formatMetric(cell.value, metric)}, ${cell.calls} calls`;
                    return (
                      <div
                        key={cell.hour}
                        className="activity-card__cell activity-card__cell--hour"
                        data-level={cell.level}
                        title={label}
                        onMouseMove={(event) => showTooltip(event, label)}
                        onMouseLeave={hideTooltip}
                      />
                    );
                  })}
                </div>
              ))}
            </div>
          </div>
        </div>

        {tooltip && (
          <div
            className="chart__tooltip activity-card__tooltip"
            style={{ left: tooltip.x, top: tooltip.y }}
            data-align={tooltip.alignment}
          >
            {tooltip.text}
          </div>
        )}
      </div>

      <footer className="activity-card__footer">
        <span className="activity-card__legend-label">Less</span>
        <span className="activity-card__legend-swatches" aria-hidden>
          {[0, 1, 2, 3, 4].map((level) => (
            <span key={level} className="activity-card__cell" data-level={level} />
          ))}
        </span>
        <span className="activity-card__legend-label">More</span>
        <span className="activity-card__footer-note">
          From local transcript timestamps. Nothing leaves this machine.
        </span>
      </footer>
    </section>
  );
}
