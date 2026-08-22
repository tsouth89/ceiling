import { useEffect, useMemo, useRef, useState } from "react";
import { getLocalActivityHeatmap } from "../lib/tauri";
import { useLocalScanRefresh } from "../hooks/useLocalScanRefresh";
import { useLocale } from "../hooks/useLocale";
import { formatLocale } from "../lib/formatLocale";
import type { ActivityHeatmap, Language } from "../types/bridge";
import type { LocaleKey } from "../i18n/keys";
import { getProviderIcon } from "./providers/providerIcons";
import {
  chartTooltipPosition,
  type ChartTooltipPosition,
} from "./charts/chartTooltip";
import {
  buildCalendar,
  buildWeekHourGrid,
  peakHour,
  peakWeekday,
  selectProviders,
  totalValue,
  type ActivityMetric,
} from "../lib/activityHeatmap";

const METRICS: { key: ActivityMetric; labelKey: LocaleKey }[] = [
  { key: "apiValue", labelKey: "ActivityHeatmapMetricApiValue" },
  { key: "tokens", labelKey: "ActivityHeatmapMetricTokens" },
];

/** Hour ticks every six hours keeps 24 columns legible at panel width. */
const HOUR_TICKS = [0, 6, 12, 18];

/**
 * Map the Settings language to an Intl locale tag (SBS-972).
 *
 * Chinese is the only shipped non-English bundle, and `locale.rs` resolves
 * every other language to the English one. They resolve to `en-US` here for
 * the same reason: `undefined` hands Intl the operating system's locale, so a
 * Japanese or Spanish selection rendered English copy alongside German dates
 * and separators on a German machine.
 */
export function activityIntlLocale(language: Language): string {
  return language === "chinese" ? "zh-CN" : "en-US";
}

/**
 * The English sentinel `get_local_activity_heatmap` returns when the worker
 * fails. Tauri hands it back as an ordinary error string, so a bare
 * `tauriErrorMessage(err) || t(...)` never reached the localized copy - the
 * sentinel is not empty, so it always won and a Chinese UI read English.
 */
const SCAN_FAILED_SENTINEL = "Unable to read local activity.";

/** Translate the sentinel and the empty rejection; pass real detail through. */
export function localizedScanError(message: string, readFailed: string): string {
  return message === "" || message === SCAN_FAILED_SENTINEL ? readFailed : message;
}

export function formatActivityUsd(value: number, locale: string | undefined): string {
  return new Intl.NumberFormat(locale, { style: "currency", currency: "USD" }).format(value);
}

export function formatActivityTokens(value: number, locale: string | undefined): string {
  return new Intl.NumberFormat(locale, {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

function formatShortDate(date: string, locale: string | undefined): string {
  const [year, month, day] = date.split("-").map(Number);
  if (!year || !month || !day) return date;
  return new Date(year, month - 1, day).toLocaleDateString(locale, {
    month: "short",
    day: "numeric",
  });
}

function formatHourLabel(hour: number, locale: string | undefined): string {
  return new Intl.DateTimeFormat(locale, { hour: "numeric" }).format(
    new Date(2020, 0, 1, hour),
  );
}

function formatWeekday(weekday: number, locale: string | undefined): string {
  // 4 January 2026 is a Sunday, matching Date.prototype.getDay() === 0.
  return new Intl.DateTimeFormat(locale, { weekday: "short" }).format(
    new Date(2026, 0, 4 + weekday),
  );
}

function providerLabel(providerId: string): string {
  return providerId.charAt(0).toUpperCase() + providerId.slice(1);
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
  const { t, language } = useLocale();
  const locale = activityIntlLocale(language);
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
        if (live) setError(tauriErrorMessage(err));
      })
      .finally(() => {
        if (live) setLoading(false);
      });
    return () => {
      live = false;
    };
    // Deliberately not keyed on `t`. LocaleProvider rebuilds it whenever the
    // bundle changes, so listing it here re-ran the fetch on every language
    // switch: the grid was replaced by the Reading spinner, and a cold cache
    // repeated the whole transcript scan the card had just finished. The
    // failure text is translated at render instead.
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

  const formatMetric = (value: number, current: ActivityMetric): string =>
    current === "apiValue"
      ? formatActivityUsd(value, locale)
      : formatLocale(t("ActivityHeatmapTokensUnit"), formatActivityTokens(value, locale));

  if (loading) {
    return (
      <section className="activity-card" aria-label={t("ActivityHeatmapAriaLabel")}>
        <p className="activity-card__status">{t("ActivityHeatmapReading")}</p>
      </section>
    );
  }

  if (error !== null) {
    return (
      <section className="activity-card" aria-label={t("ActivityHeatmapAriaLabel")}>
        <p className="activity-card__status">{localizedScanError(error, t("ActivityHeatmapReadFailed"))}</p>
      </section>
    );
  }

  const hasActivity = rows.length > 0;
  // An empty grid because every chip is off is a different thing from an empty
  // grid because the machine was idle, and saying the wrong one reads as a bug.
  const allProvidersHidden = providerIds.length > 0 && visibleProviders.length === 0;

  return (
    <section className="activity-card" aria-label={t("ActivityHeatmapAriaLabel")}>
      <header className="activity-card__header">
        <div>
          <h3 className="activity-card__title">{t("ActivityHeatmapTitle")}</h3>
          <p className="activity-card__subtitle">
            {formatLocale(
              t("ActivityHeatmapSubtitle"),
              String(calendar.length),
              heatmap?.timezoneLabel ?? t("ActivityHeatmapLocalTime"),
            )}
          </p>
        </div>
        <div className="activity-card__switch" role="group" aria-label={t("ActivityHeatmapMetricGroup")}>
          {METRICS.map((option) => (
            <button
              key={option.key}
              type="button"
              className="activity-card__switch-btn"
              data-active={metric === option.key ? "true" : "false"}
              aria-pressed={metric === option.key}
              onClick={() => setMetric(option.key)}
            >
              {t(option.labelKey)}
            </button>
          ))}
        </div>
      </header>

      {providerIds.length > 1 && (
        <div className="activity-card__providers" role="group" aria-label={t("ActivityHeatmapProvidersGroup")}>
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
              ? t("ActivityHeatmapProvidersHidden")
              : t("ActivityHeatmapEmpty")}
          </p>
        )}

        <div className="activity-card__section">
          <div className="activity-card__section-head">
            <span className="activity-card__section-title">{t("ActivityHeatmapByDay")}</span>
            <span className="activity-card__section-note">
              {formatLocale(t("ActivityHeatmapTotal"), formatMetric(total, metric))}
            </span>
          </div>
          <div
            className="activity-card__days"
            role="img"
            aria-label={formatLocale(t("ActivityHeatmapDaysAria"), String(calendar.length))}
          >
            {calendar.map((cell) => {
              const label = formatLocale(
                t("ActivityHeatmapCellSummary"),
                formatShortDate(cell.date, locale),
                formatMetric(cell.value, metric),
                String(cell.calls),
              );
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
              <span>{formatShortDate(calendar[0].date, locale)}</span>
              <span>{formatShortDate(calendar[calendar.length - 1].date, locale)}</span>
            </div>
          )}
        </div>

        <div className="activity-card__section">
          <div className="activity-card__section-head">
            <span className="activity-card__section-title">{t("ActivityHeatmapByHour")}</span>
            <span className="activity-card__section-note">
              {busiestHour && busiestWeekday
                ? formatLocale(
                    t("ActivityHeatmapBusiest"),
                    formatWeekday(busiestWeekday.weekday, locale),
                    formatHourLabel(busiestHour.hour, locale),
                  )
                : t("ActivityHeatmapNoPeak")}
            </span>
          </div>
          <div className="activity-card__grid-wrap">
            <div className="activity-card__hour-axis" aria-hidden>
              {HOUR_TICKS.map((hour) => (
                <span key={hour} style={{ gridColumn: `${hour + 1} / span 6` }}>
                  {formatHourLabel(hour, locale)}
                </span>
              ))}
            </div>
            <div
              className="activity-card__grid"
              role="img"
              aria-label={t("ActivityHeatmapHoursAria")}
            >
              {grid.map((row, weekday) => (
                <div className="activity-card__grid-row" key={weekday}>
                  <span className="activity-card__weekday" aria-hidden>
                    {formatWeekday(weekday, locale)}
                  </span>
                  {row.map((cell) => {
                    const label = formatLocale(
                      t("ActivityHeatmapHourCell"),
                      formatWeekday(weekday, locale),
                      formatHourLabel(cell.hour, locale),
                      formatMetric(cell.value, metric),
                      String(cell.calls),
                    );
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
        <span className="activity-card__legend-label">{t("ActivityHeatmapLess")}</span>
        <span className="activity-card__legend-swatches" aria-hidden>
          {[0, 1, 2, 3, 4].map((level) => (
            <span key={level} className="activity-card__cell" data-level={level} />
          ))}
        </span>
        <span className="activity-card__legend-label">{t("ActivityHeatmapMore")}</span>
        <span className="activity-card__footer-note">
          {t("ActivityHeatmapFooterNote")}
        </span>
      </footer>
    </section>
  );
}
