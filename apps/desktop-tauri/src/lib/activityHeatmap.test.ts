import { describe, expect, it } from "vitest";
import type { ActivityHourPoint } from "../types/bridge";
import {
  bandThresholds,
  buildCalendar,
  buildWeekHourGrid,
  formatHourLabel,
  intensityLevel,
  parseLocalDate,
  peakHour,
  peakWeekday,
  selectProviders,
  totalValue,
} from "./activityHeatmap";

const point = (over: Partial<ActivityHourPoint>): ActivityHourPoint => ({
  providerId: "codex",
  date: "2026-08-10",
  hour: 9,
  apiValueUsd: 1,
  tokens: 100,
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
    expect(bands).toEqual([3, 5, 7]);
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
  });

  it("ignores empty cells when choosing the bands", () => {
    expect(bandThresholds([0, 0, 0])).toEqual([0, 0, 0]);
    // Only 4 and 8 are active, so the quartiles sit on those two values alone
    // rather than being dragged toward zero by the empty cells.
    expect(bandThresholds([0, 0, 4, 8])).toEqual([4, 8, 8]);
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
