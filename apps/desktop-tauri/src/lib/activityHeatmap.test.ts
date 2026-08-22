import { describe, expect, it } from "vitest";
import type { ActivityHourPoint } from "../types/bridge";
import {
  bandThresholds,
  buildCalendar,
  buildWeekHourGrid,
  formatHourLabel,
  intensityLevel,
  isEmptyHeatmap,
  parseLocalDate,
  peakHour,
  peakWeekday,
  priceState,
  pricingCoverage,
  pricingCoverageNote,
  selectProviders,
  totalValue,
} from "./activityHeatmap";

const point = (over: Partial<ActivityHourPoint>): ActivityHourPoint => ({
  providerId: "codex",
  date: "2026-08-10",
  hour: 9,
  apiValueUsd: 1,
  tokens: 100,
  pricedTokens: 100,
  totalTokens: 100,
  calls: 1,
  ...over,
});

describe("parseLocalDate", () => {
  // `new Date("2026-08-15")` is UTC midnight, which is Aug 14 anywhere west of
  // Greenwich. That would shift every cell one column.
  it("reads the date on the local clock, not UTC", () => {
    const parsed = parseLocalDate("2026-08-15");
    expect(parsed?.getFullYear()).toBe(2026);
    expect(parsed?.getMonth()).toBe(7);
    expect(parsed?.getDate()).toBe(15);
  });

  it("rejects malformed dates", () => {
    expect(parseLocalDate("")).toBeNull();
    expect(parseLocalDate("not-a-date")).toBeNull();
  });
});

describe("intensity banding", () => {
  it("bands by quartile so one big session cannot flatten the rest", () => {
    // A linear scale against the 1000 peak would put all eight ordinary values
    // in the bottom band and show nothing about the ordinary days.
    const bands = bandThresholds([1, 2, 3, 4, 5, 6, 7, 8, 1000]);
    expect(bands).toEqual([3, 5, 7, 1000]);
    expect(intensityLevel(1, bands)).toBe(1);
    expect(intensityLevel(4, bands)).toBe(2);
    expect(intensityLevel(6, bands)).toBe(3);
    expect(intensityLevel(1000, bands)).toBe(4);
  });

  it("treats zero and negative as no activity", () => {
    const bands = bandThresholds([5, 10, 15]);
    expect(intensityLevel(0, bands)).toBe(0);
    expect(intensityLevel(-1, bands)).toBe(0);
  });

  it("holds a flat distribution mid-scale instead of all-peak", () => {
    const bands = bandThresholds([7, 7, 7, 7]);
    expect(intensityLevel(7, bands)).toBe(2);
    // The collapsed-cut shortcut must not also swallow an outlier sitting
    // above those ties. Before SBS-945, `if (low === high) return 2` painted
    // 500 the same shade as 7.
    expect(intensityLevel(500, bands)).toBe(4);
  });

  /// 29 quiet days and one spike: every quartile element is the quiet value,
  /// so low === high. The spike must still reach the top swatch.
  it("does not flatten an outlier to the quiet majority's shade", () => {
    const values = [...Array.from({ length: 29 }, () => 1), 500];
    const bands = bandThresholds(values);
    expect(bands).toEqual([1, 1, 1, 500]);
    expect(intensityLevel(1, bands)).toBe(2);
    expect(intensityLevel(500, bands)).toBe(4);
  });

  it("lets the maximum reach the top swatch with two or three active cells", () => {
    expect(intensityLevel(100, bandThresholds([1, 100]))).toBe(4);
    expect(intensityLevel(1, bandThresholds([1, 100]))).toBeLessThan(4);
    expect(intensityLevel(100, bandThresholds([1, 50, 100]))).toBe(4);
    expect(intensityLevel(1, bandThresholds([1, 50, 100]))).toBeLessThan(4);
  });

  /// SBS-945: the top swatch is the busiest cell, in both directions. Deciding
  /// it from the 75th-percentile cut painted a tied maximum mid-scale, and
  /// handed the same shade to a distinct runner-up.
  it("gives the top swatch to tied peaks but not to a distinct runner-up", () => {
    // Upper half ties: cuts land at [1, 50, 50], so `value <= mid` used to
    // catch the busiest cell and paint it level 2.
    const tied = bandThresholds([1, 50, 50]);
    expect(tied).toEqual([1, 50, 50, 50]);
    expect(intensityLevel(50, tied)).toBe(4);
    expect(intensityLevel(1, tied)).toBe(1);

    // Median equal to Q3, the same shape with a larger peak.
    const tiedHigh = bandThresholds([1, 100, 100]);
    expect(intensityLevel(100, tiedHigh)).toBe(4);

    // Three of five tied at the top.
    const tiedRun = bandThresholds([1, 1, 10, 10, 10]);
    expect(intensityLevel(10, tiedRun)).toBe(4);
    expect(intensityLevel(1, tiedRun)).toBe(1);

    // Four distinct cells: Q3 is 100, so `value < high` used to give 100 and
    // 200 the same shade. Only the busiest is the peak.
    const distinct = bandThresholds([1, 50, 100, 200]);
    expect(distinct).toEqual([1, 50, 100, 200]);
    expect(intensityLevel(200, distinct)).toBe(4);
    expect(intensityLevel(100, distinct)).toBe(3);
    expect(intensityLevel(50, distinct)).toBe(2);
    expect(intensityLevel(1, distinct)).toBe(1);
  });

  it("ignores empty cells when choosing the bands", () => {
    expect(bandThresholds([0, 0, 0])).toEqual([0, 0, 0, 0]);
    // Only 4 and 8 are active, so the quartiles sit on those two values alone
    // rather than being dragged toward zero by the empty cells.
    expect(bandThresholds([0, 0, 4, 8])).toEqual([4, 4, 8, 8]);
  });
});

describe("buildCalendar", () => {
  const days = ["2026-08-08", "2026-08-09", "2026-08-10"];

  it("keeps empty days so the grid stays continuous", () => {
    const cells = buildCalendar(days, [point({ date: "2026-08-10" })], "apiValue");
    expect(cells.map((cell) => cell.date)).toEqual(days);
    expect(cells[0].level).toBe(0);
    expect(cells[0].value).toBe(0);
    expect(cells[2].level).toBeGreaterThan(0);
  });

  it("sums every hour of a day, across providers", () => {
    const cells = buildCalendar(
      days,
      [
        point({ date: "2026-08-09", hour: 9, apiValueUsd: 2, calls: 3 }),
        point({ date: "2026-08-09", hour: 14, apiValueUsd: 5, calls: 4 }),
        point({ providerId: "claude", date: "2026-08-09", hour: 14, apiValueUsd: 1, calls: 1 }),
      ],
      "apiValue",
    );
    expect(cells[1].value).toBe(8);
    expect(cells[1].calls).toBe(8);
  });

  it("switches metric without touching the shape", () => {
    const hours = [point({ date: "2026-08-10", apiValueUsd: 2, tokens: 900 })];
    expect(buildCalendar(days, hours, "apiValue")[2].value).toBe(2);
    expect(buildCalendar(days, hours, "tokens")[2].value).toBe(900);
  });

  /// Calendar path of SBS-945: 29 quiet days plus one spike must not share a
  /// shade. Empty days stay 0 so they are not confused with quiet activity.
  it("paints a spike day darker than a month of quiet days", () => {
    const month = Array.from({ length: 30 }, (_, index) => {
      const day = String(index + 1).padStart(2, "0");
      return `2026-08-${day}`;
    });
    const hours = month.map((date, index) =>
      point({ date, apiValueUsd: index === 29 ? 500 : 1, tokens: 1, calls: 1 }),
    );
    const cells = buildCalendar(month, hours, "apiValue");
    expect(cells).toHaveLength(30);
    expect(cells[0].level).toBe(2);
    expect(cells[28].level).toBe(2);
    expect(cells[29].level).toBe(4);
  });
});

describe("buildWeekHourGrid", () => {
  it("is always a full 7 x 24 grid", () => {
    const grid = buildWeekHourGrid([], "apiValue");
    expect(grid).toHaveLength(7);
    expect(grid.every((row) => row.length === 24)).toBe(true);
    expect(grid.flat().every((cell) => cell.level === 0)).toBe(true);
  });

  it("folds the same weekday and hour across weeks into one cell", () => {
    // 2026-08-03 and 2026-08-10 are both Mondays.
    const grid = buildWeekHourGrid(
      [
        point({ date: "2026-08-03", hour: 15, apiValueUsd: 4, calls: 2 }),
        point({ date: "2026-08-10", hour: 15, apiValueUsd: 6, calls: 3 }),
      ],
      "apiValue",
    );
    const monday = grid[1];
    expect(monday[15].value).toBe(10);
    expect(monday[15].calls).toBe(5);
    expect(monday[14].value).toBe(0);
  });

  it("bands across the whole grid, not per row", () => {
    // A quiet Sunday must stay visibly quieter than a busy Monday.
    const grid = buildWeekHourGrid(
      [
        point({ date: "2026-08-09", hour: 2, apiValueUsd: 1 }),
        point({ date: "2026-08-10", hour: 2, apiValueUsd: 50 }),
        point({ date: "2026-08-11", hour: 2, apiValueUsd: 100 }),
        point({ date: "2026-08-12", hour: 2, apiValueUsd: 200 }),
      ],
      "apiValue",
    );
    expect(grid[0][2].level).toBeLessThan(grid[3][2].level);
    // Four active cells used to put the 75th-percentile cut on the maximum,
    // so the busiest cell stopped at level 3 and the legend's top swatch
    // never appeared. $200 must reach 4 (SBS-945).
    expect(grid[0][2].level).toBe(1);
    expect(grid[3][2].level).toBe(4);
  });
});

describe("selectProviders", () => {
  const hours = [point({ providerId: "codex" }), point({ providerId: "claude" })];

  it("filters to the visible providers", () => {
    expect(selectProviders(hours, ["claude"]).map((row) => row.providerId)).toEqual(["claude"]);
    expect(selectProviders(hours, ["codex", "claude"])).toHaveLength(2);
  });

  // Turning every chip off must empty the grid. Treating an empty list as
  // "no filter" showed every provider instead, which reads as the filter
  // being broken.
  it("shows nothing when no provider is visible", () => {
    expect(selectProviders(hours, [])).toEqual([]);
  });
});

describe("peaks", () => {
  const hours = [
    point({ date: "2026-08-10", hour: 9, apiValueUsd: 3 }),
    point({ date: "2026-08-11", hour: 22, apiValueUsd: 11 }),
    point({ date: "2026-08-12", hour: 22, apiValueUsd: 4 }),
  ];

  it("finds the busiest clock hour and weekday", () => {
    expect(peakHour(hours, "apiValue")).toEqual({ hour: 22, value: 15 });
    // 2026-08-11 is a Tuesday.
    expect(peakWeekday(hours, "apiValue")).toEqual({ weekday: 2, value: 11 });
  });

  it("reports the earlier hour on an exact tie", () => {
    const tied = [
      point({ date: "2026-08-10", hour: 20, apiValueUsd: 5 }),
      point({ date: "2026-08-10", hour: 8, apiValueUsd: 5 }),
    ];
    expect(peakHour(tied, "apiValue")?.hour).toBe(8);
  });

  it("has no peak without activity", () => {
    expect(peakHour([], "apiValue")).toBeNull();
    expect(peakWeekday([], "apiValue")).toBeNull();
    expect(peakHour([point({ apiValueUsd: 0 })], "apiValue")).toBeNull();
  });
});

describe("totalValue", () => {
  it("sums the selected metric", () => {
    const hours = [point({ apiValueUsd: 1.5, tokens: 10 }), point({ apiValueUsd: 2.5, tokens: 20 })];
    expect(totalValue(hours, "apiValue")).toBe(4);
    expect(totalValue(hours, "tokens")).toBe(30);
  });
});

describe("formatHourLabel", () => {
  it("reads as a wall clock", () => {
    expect(formatHourLabel(0)).toBe("12 AM");
    expect(formatHourLabel(9)).toBe("9 AM");
    expect(formatHourLabel(12)).toBe("12 PM");
    expect(formatHourLabel(23)).toBe("11 PM");
  });
});

describe("pricing coverage (SBS-952)", () => {
  // A no-price provider used to paint every cell at intensity 0, print
  // "$0.00 total", and drop the busiest-hour line. That is idle, not unknown.

  it("keeps an all-unpriced month out of the idle state", () => {
    const hours = [
      point({
        date: "2026-08-10",
        hour: 9,
        apiValueUsd: 0,
        tokens: 8_000,
        pricedTokens: 0,
        totalTokens: 8_000,
        calls: 12,
      }),
      point({
        date: "2026-08-11",
        hour: 14,
        apiValueUsd: 0,
        tokens: 2_000,
        pricedTokens: 0,
        totalTokens: 2_000,
        calls: 3,
      }),
    ];

    const coverage = pricingCoverage(hours);
    expect(coverage.pricedTokens).toBe(0);
    expect(coverage.totalTokens).toBe(10_000);
    expect(coverage.coverage).toBe(0);
    expect(coverage.unpricedProviderIds).toEqual(["codex"]);
    expect(pricingCoverageNote(coverage)).toBe(
      "0% of tokens priced (unpriced models in Codex)",
    );

    expect(isEmptyHeatmap({ days: ["2026-08-10"], providerIds: ["codex"], hours, timezoneLabel: "UTC" })).toBe(
      false,
    );
    expect(peakHour(hours, "apiValue")).toBeNull();
    expect(peakHour(hours, "tokens")).toEqual({ hour: 9, value: 8_000 });

    const days = ["2026-08-10", "2026-08-11"];
    const calendar = buildCalendar(days, hours, "apiValue");
    expect(calendar[0].level).toBe(0);
    expect(calendar[0].value).toBe(0);
    expect(calendar[0].totalTokens).toBe(8_000);
    expect(priceState(calendar[0])).toBe("unpriced");
    expect(priceState(calendar[1])).toBe("unpriced");
    expect(priceState({ pricedTokens: 0, totalTokens: 0 })).toBe("idle");
  });

  it("speaks only when some tokens are unpriced", () => {
    const full = pricingCoverage([point({ pricedTokens: 100, totalTokens: 100 })]);
    expect(full.coverage).toBe(1);
    expect(pricingCoverageNote(full)).toBeNull();

    const empty = pricingCoverage([]);
    expect(empty.coverage).toBeNull();
    expect(pricingCoverageNote(empty)).toBeNull();

    const mixed = pricingCoverage([
      point({ providerId: "codex", pricedTokens: 400, totalTokens: 1_000 }),
      point({ providerId: "claude", pricedTokens: 500, totalTokens: 500 }),
    ]);
    expect(mixed.coverage).toBeCloseTo(0.6);
    expect(mixed.unpricedProviderIds).toEqual(["codex"]);
    expect(pricingCoverageNote(mixed)).toBe(
      "60% of tokens priced (unpriced models in Codex)",
    );
  });

  it("does not treat missing coverage as idle when the Tokens metric has work", () => {
    const hours = [
      point({ apiValueUsd: 0, tokens: 900, pricedTokens: 0, totalTokens: 900 }),
    ];
    expect(totalValue(hours, "apiValue")).toBe(0);
    expect(totalValue(hours, "tokens")).toBe(900);
    expect(buildCalendar(["2026-08-10"], hours, "tokens")[0].level).toBeGreaterThan(0);
  });
});
