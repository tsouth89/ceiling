/**
 * Pure aggregation behind the activity heatmap card (SBS-277).
 *
 * The backend sends one row per (provider, local day, local hour) that saw
 * activity. Everything here reshapes those rows into the two grids the card
 * draws and decides each cell's intensity band. Kept out of the component so
 * the banding rules are testable without rendering.
 */

import type { ActivityHeatmap, ActivityHourPoint } from "../types/bridge";

export type ActivityMetric = "apiValue" | "tokens";

/** 0 means no activity; 1-4 are the sequential ramp's steps. */
export type IntensityLevel = 0 | 1 | 2 | 3 | 4;

export type CellTotals = {
  value: number;
  calls: number;
};

export type CalendarCell = CellTotals & {
  /** Local calendar date as `YYYY-MM-DD`. */
  date: string;
  level: IntensityLevel;
};

export type HourCell = CellTotals & {
  /** 0 = Sunday, matching `Date.prototype.getDay`. */
  weekday: number;
  /** Local hour of day, 0-23. */
  hour: number;
  level: IntensityLevel;
};

/** Quartile cut points over the active cells, low to high. */
export type Bands = [number, number, number];

const EMPTY: CellTotals = { value: 0, calls: 0 };

export const WEEKDAY_LABELS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** Read the selected metric off a backend row. */
function metricValue(point: ActivityHourPoint, metric: ActivityMetric): number {
  return metric === "apiValue" ? point.apiValueUsd : point.tokens;
}

/**
 * Parse `YYYY-MM-DD` as a local calendar date.
 *
 * `new Date("2026-08-15")` is parsed as UTC midnight and can land on the
 * previous day west of Greenwich, which would shift the whole calendar by a
 * column. Building from parts keeps the date on the local clock the backend
 * already bucketed by.
 */
export function parseLocalDate(date: string): Date | null {
  const [year, month, day] = date.split("-").map(Number);
  if (!year || !month || !day) return null;
  const parsed = new Date(year, month - 1, day);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

/**
 * Quartile cut points over active cells only.
 *
 * Activity is heavily skewed: one long session can be worth more than the rest
 * of the month. Linear banding against the peak would paint every ordinary day
 * at level 1 and tell the reader nothing, so the bands follow the distribution
 * instead.
 */
export function bandThresholds(values: number[]): Bands {
  const active = values.filter((value) => value > 0).sort((left, right) => left - right);
  if (active.length === 0) return [0, 0, 0];
  const n = active.length;
  // Inclusive rank: ceil(q*n)-1, not floor(q*n). floor(0.75*n) is n-1 for
  // n of 2, 3 or 4, so the 75th-percentile cut sat on the maximum and the
  // busiest cell could never reach the legend's top swatch (SBS-945).
  const at = (quantile: number) =>
    active[Math.min(n - 1, Math.max(0, Math.ceil(quantile * n) - 1))];
  return [at(0.25), at(0.5), at(0.75)];
}

export function intensityLevel(value: number, bands: Bands): IntensityLevel {
  if (value <= 0) return 0;
  const [low, mid, high] = bands;
  // A genuinely flat distribution has no bands to show. Painting every active
  // cell at the top would read as "every day was a peak", so hold the mass
  // mid-scale. The shortcut applies only to values at or below that collapsed
  // cut: a single outlier above it still reaches the top swatch, which is the
  // case the quartile bands exist to surface (SBS-945).
  if (low === high && value <= high) return 2;
  if (value <= low) return 1;
  if (value <= mid) return 2;
  // Exclusive upper bound so a cell at the 75th-percentile value — including
  // the maximum when the top quartile is all ties at the max — still paints
  // as the legend's darkest step rather than one below it.
  if (value < high) return 3;
  return 4;
}

function addTotals(target: Map<string, CellTotals>, key: string, value: number, calls: number) {
  const current = target.get(key) ?? { value: 0, calls: 0 };
  target.set(key, { value: current.value + value, calls: current.calls + calls });
}

/**
 * Rows for the visible providers.
 *
 * An empty list means nothing is visible, not everything. The caller passes the
 * true visible set, so treating empty as "no filter" would make turning every
 * provider chip off show every provider instead of an empty grid.
 */
export function selectProviders(
  hours: ActivityHourPoint[],
  providerIds: string[],
): ActivityHourPoint[] {
  const wanted = new Set(providerIds);
  return hours.filter((point) => wanted.has(point.providerId));
}

/**
 * One cell per local calendar day, oldest first, gaps included.
 *
 * `days` comes from the backend so a month with no activity at all still draws
 * a full grid rather than collapsing to nothing.
 */
export function buildCalendar(
  days: string[],
  hours: ActivityHourPoint[],
  metric: ActivityMetric,
): CalendarCell[] {
  const totals = new Map<string, CellTotals>();
  for (const point of hours) {
    addTotals(totals, point.date, metricValue(point, metric), point.calls);
  }
  const bands = bandThresholds(days.map((date) => totals.get(date)?.value ?? 0));
  return days.map((date) => {
    const cell = totals.get(date) ?? EMPTY;
    return { date, ...cell, level: intensityLevel(cell.value, bands) };
  });
}

/**
 * Weekday x hour grid: 7 rows of 24 cells, Sunday first.
 *
 * This is the peak-hours view. Bands are computed across the whole grid so a
 * quiet Sunday reads as quiet next to a busy Wednesday, rather than each row
 * being normalized against itself.
 */
export function buildWeekHourGrid(
  hours: ActivityHourPoint[],
  metric: ActivityMetric,
): HourCell[][] {
  const totals = new Map<string, CellTotals>();
  for (const point of hours) {
    const date = parseLocalDate(point.date);
    if (!date) continue;
    addTotals(
      totals,
      `${date.getDay()}:${point.hour}`,
      metricValue(point, metric),
      point.calls,
    );
  }
  const bands = bandThresholds([...totals.values()].map((cell) => cell.value));
  return WEEKDAY_LABELS.map((_, weekday) =>
    Array.from({ length: 24 }, (_, hour) => {
      const cell = totals.get(`${weekday}:${hour}`) ?? EMPTY;
      return { weekday, hour, ...cell, level: intensityLevel(cell.value, bands) };
    }),
  );
}

/** Total for the selected metric across every cell. */
export function totalValue(hours: ActivityHourPoint[], metric: ActivityMetric): number {
  return hours.reduce((sum, point) => sum + metricValue(point, metric), 0);
}

/**
 * Busiest clock hour across the whole range, or `null` with no activity.
 *
 * Stated in words next to the grid so the peak is never carried by color
 * alone.
 */
export function peakHour(
  hours: ActivityHourPoint[],
  metric: ActivityMetric,
): { hour: number; value: number } | null {
  const totals = new Map<number, number>();
  for (const point of hours) {
    totals.set(point.hour, (totals.get(point.hour) ?? 0) + metricValue(point, metric));
  }
  let best: { hour: number; value: number } | null = null;
  // Ascending hour order so an exact tie reports the earlier hour rather than
  // whichever the map happened to yield first.
  for (const hour of [...totals.keys()].sort((left, right) => left - right)) {
    const value = totals.get(hour) ?? 0;
    if (value > 0 && (best === null || value > best.value)) best = { hour, value };
  }
  return best;
}

/** Busiest weekday across the whole range, or `null` with no activity. */
export function peakWeekday(
  hours: ActivityHourPoint[],
  metric: ActivityMetric,
): { weekday: number; value: number } | null {
  const totals = new Map<number, number>();
  for (const point of hours) {
    const date = parseLocalDate(point.date);
    if (!date) continue;
    const weekday = date.getDay();
    totals.set(weekday, (totals.get(weekday) ?? 0) + metricValue(point, metric));
  }
  let best: { weekday: number; value: number } | null = null;
  for (const weekday of [...totals.keys()].sort((left, right) => left - right)) {
    const value = totals.get(weekday) ?? 0;
    if (value > 0 && (best === null || value > best.value)) best = { weekday, value };
  }
  return best;
}

/** `14` reads as "2 PM"; used in labels and tooltips, never just a color. */
export function formatHourLabel(hour: number): string {
  const suffix = hour < 12 ? "AM" : "PM";
  const display = hour % 12 === 0 ? 12 : hour % 12;
  return `${display} ${suffix}`;
}

/** True when the payload has nothing worth drawing. */
export function isEmptyHeatmap(heatmap: ActivityHeatmap | null): boolean {
  return !heatmap || heatmap.hours.length === 0;
}
