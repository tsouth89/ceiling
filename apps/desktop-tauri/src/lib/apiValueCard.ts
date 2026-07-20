import type { LocalApiValuePeriod, LocalApiValueProvider } from "../types/bridge";

/** Metric the aggregate card is showing. */
export type ApiValueMetric = "apiValue" | "tokens";

/** Which of the three periods is selected. */
export type ApiValuePeriodKey = "today" | "yesterday" | "thirtyDays";

export interface ApiValueSlice {
  providerId: string;
  /** Metric value for the provider (USD for apiValue, tokens for tokens). */
  value: number;
  /** Fraction of the period total, 0..1 (0 when the total is 0). */
  share: number;
}

export interface ApiValueRingSegment {
  providerId: string;
  /** Length of the colored arc along the ring. */
  dash: number;
  /** SVG stroke-dashoffset placing this segment after the previous ones. */
  offset: number;
}

/** Dollar (or token) change vs the comparable prior window. */
export interface ApiValuePeriodChange {
  /** Percent change vs prior; null when prior had no data. */
  percent: number | null;
  /** Short label, e.g. "vs yesterday" or "vs prior 30d". */
  versusLabel: string;
  /** True when prior window had data so a % is meaningful. */
  hasPrior: boolean;
}

export interface ApiValueCardModel {
  /** Providers with data this period, sorted by value desc, with shares. */
  slices: ApiValueSlice[];
  /** Sum of the selected metric across contributing providers. */
  total: number;
  /** Priced model tokens summed across contributing providers. */
  pricedTokens: number;
  /** All model tokens (priced + unpriced) across contributing providers. */
  totalTokens: number;
  /** pricedTokens / totalTokens, or null when there are no model tokens. */
  coverage: number | null;
  /** Providers that have unpriced tokens this period (for the details note). */
  unpricedProviderIds: string[];
  /** True when no provider had any data this period ("No data" state). */
  isEmpty: boolean;
  /** Period-over-period delta for the selected metric (apiValue or tokens). */
  periodChange: ApiValuePeriodChange | null;
}

function periodOf(
  provider: LocalApiValueProvider,
  key: ApiValuePeriodKey,
): LocalApiValuePeriod {
  if (key === "today") return provider.today;
  if (key === "yesterday") return provider.yesterday;
  return provider.thirtyDays;
}

function priorPeriodOf(
  provider: LocalApiValueProvider,
  key: ApiValuePeriodKey,
): { period: LocalApiValuePeriod; versusLabel: string } | null {
  if (key === "today") {
    return { period: provider.yesterday, versusLabel: "vs yesterday" };
  }
  if (key === "thirtyDays") {
    return { period: provider.priorThirtyDays, versusLabel: "vs prior 30d" };
  }
  // Yesterday has no stable prior on this card.
  return null;
}

function metricValue(period: LocalApiValuePeriod, metric: ApiValueMetric): number {
  return metric === "apiValue" ? period.apiValueUsd : period.tokens;
}

function sumMetric(
  providers: LocalApiValueProvider[],
  pick: (provider: LocalApiValueProvider) => LocalApiValuePeriod,
  metric: ApiValueMetric,
): { total: number; hasData: boolean } {
  let total = 0;
  let hasData = false;
  for (const provider of providers) {
    const period = pick(provider);
    if (!period.hasData) continue;
    hasData = true;
    total += metricValue(period, metric);
  }
  return { total, hasData };
}

/**
 * Build the card model for one period + metric. Providers without data this
 * period are omitted (never counted as zero); shares are of the metric total.
 * Pricing coverage is always computed from model tokens, independent of the
 * selected metric, so the transparency note is consistent across metrics.
 */
export function buildApiValueCard(
  providers: LocalApiValueProvider[],
  periodKey: ApiValuePeriodKey,
  metric: ApiValueMetric,
): ApiValueCardModel {
  const rows = providers
    .map((provider) => ({ provider, period: periodOf(provider, periodKey) }))
    .filter(({ period }) => period.hasData);

  const total = rows.reduce((sum, { period }) => sum + metricValue(period, metric), 0);
  const slices: ApiValueSlice[] = rows
    .map(({ provider, period }) => ({
      providerId: provider.providerId,
      value: metricValue(period, metric),
    }))
    .sort((a, b) => b.value - a.value || a.providerId.localeCompare(b.providerId))
    .map((slice) => ({ ...slice, share: total > 0 ? slice.value / total : 0 }));

  const pricedTokens = rows.reduce((sum, { period }) => sum + period.pricedTokens, 0);
  const totalTokens = rows.reduce((sum, { period }) => sum + period.totalTokens, 0);
  const unpricedProviderIds = rows
    .filter(({ period }) => period.totalTokens > period.pricedTokens)
    .map(({ provider }) => provider.providerId);

  const priorMeta = rows.length > 0 ? priorPeriodOf(providers[0], periodKey) : null;
  let periodChange: ApiValuePeriodChange | null = null;
  if (priorMeta) {
    const priorSum = sumMetric(
      providers,
      (provider) => priorPeriodOf(provider, periodKey)!.period,
      metric,
    );
    if (!priorSum.hasData) {
      periodChange = {
        versusLabel: priorMeta.versusLabel,
        hasPrior: false,
        percent: null,
      };
    } else if (priorSum.total <= 0) {
      periodChange = {
        versusLabel: priorMeta.versusLabel,
        hasPrior: true,
        percent: total > 0 ? null : 0,
      };
    } else {
      periodChange = {
        versusLabel: priorMeta.versusLabel,
        hasPrior: true,
        percent: ((total - priorSum.total) / priorSum.total) * 100,
      };
    }
  }

  return {
    slices,
    total,
    pricedTokens,
    totalTokens,
    coverage: totalTokens > 0 ? pricedTokens / totalTokens : null,
    unpricedProviderIds,
    isEmpty: rows.length === 0,
    periodChange: rows.length === 0 ? null : periodChange,
  };
}

/**
 * Turn slice shares into SVG ring segments (the stroke-dasharray donut
 * technique): each segment's `dash` is its arc length and `offset` places it
 * after the previous segments around the circumference.
 */
export function ringSegments(
  slices: ApiValueSlice[],
  circumference: number,
): ApiValueRingSegment[] {
  let cumulative = 0;
  return slices.map((slice) => {
    const segment: ApiValueRingSegment = {
      providerId: slice.providerId,
      dash: slice.share * circumference,
      offset: -cumulative * circumference,
    };
    cumulative += slice.share;
    return segment;
  });
}

/** Format a period-change for the donut center caption. */
export function formatPeriodChange(change: ApiValuePeriodChange): string | null {
  if (!change.hasPrior) return null;
  if (change.percent == null) return `New activity ${change.versusLabel}`;
  const rounded = Math.round(change.percent);
  const sign = rounded > 0 ? "+" : "";
  return `${sign}${rounded}% ${change.versusLabel}`;
}
