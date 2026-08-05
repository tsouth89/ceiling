import { describe, expect, it } from "vitest";

import type { RateWindowSnapshot } from "../types/bridge";
import { expectedOverlay, expectedUsedPercent } from "./expectedPace";

const NOW = Date.parse("2026-08-05T12:00:00.000Z");
const MINUTE = 60 * 1000;

function win(overrides: Partial<RateWindowSnapshot> = {}): RateWindowSnapshot {
  return {
    usedPercent: 50,
    remainingPercent: 50,
    windowMinutes: 10080,
    resetsAt: new Date(NOW + 3.5 * 24 * 60 * MINUTE).toISOString(),
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
    reserveWillLastToReset: false,
    reserveEtaSeconds: null,
    ...overrides,
  };
}

describe("expectedUsedPercent", () => {
  it("is the elapsed fraction of the window", () => {
    // Weekly window resetting in 3.5 days => exactly half elapsed.
    expect(expectedUsedPercent(win(), NOW)).toBeCloseTo(50, 5);
  });

  it("tracks a monthly window", () => {
    const monthly = win({
      windowMinutes: 30 * 24 * 60,
      resetsAt: new Date(NOW + 6 * 24 * 60 * MINUTE).toISOString(),
    });
    // 24 of 30 days elapsed.
    expect(expectedUsedPercent(monthly, NOW)).toBeCloseTo(80, 5);
  });

  it("stays silent on short windows, where an even burn means nothing", () => {
    // A 5-hour session window: nobody paces one linearly.
    const session = win({
      windowMinutes: 300,
      resetsAt: new Date(NOW + 150 * MINUTE).toISOString(),
    });
    expect(expectedUsedPercent(session, NOW)).toBeNull();
  });

  it("needs both a duration and a reset to place the marker", () => {
    expect(expectedUsedPercent(win({ windowMinutes: null }), NOW)).toBeNull();
    expect(expectedUsedPercent(win({ resetsAt: null }), NOW)).toBeNull();
    expect(expectedUsedPercent(win({ resetsAt: "not-a-date" }), NOW)).toBeNull();
    expect(expectedUsedPercent(win({ windowMinutes: 0 }), NOW)).toBeNull();
  });

  it("stays silent outside the window rather than pinning to an edge", () => {
    // Already reset.
    const past = win({ resetsAt: new Date(NOW - MINUTE).toISOString() });
    expect(expectedUsedPercent(past, NOW)).toBeNull();
    // Reset further out than the window is long: the window has not started.
    const future = win({
      resetsAt: new Date(NOW + 30 * 24 * 60 * MINUTE).toISOString(),
    });
    expect(expectedUsedPercent(future, NOW)).toBeNull();
  });
});

describe("expectedOverlay", () => {
  // 80% of a monthly window elapsed.
  const monthly = (usedPercent: number) =>
    win({
      usedPercent,
      remainingPercent: 100 - usedPercent,
      windowMinutes: 30 * 24 * 60,
      resetsAt: new Date(NOW + 6 * 24 * 60 * MINUTE).toISOString(),
    });

  it("mirrors the marker when the bar shows remaining capacity", () => {
    expect(expectedOverlay(monthly(80), true, NOW)?.tickPercent).toBeCloseTo(80, 5);
    expect(expectedOverlay(monthly(80), false, NOW)?.tickPercent).toBeCloseTo(20, 5);
  });

  it("reads 'ahead' from usage, not from bar geometry", () => {
    // Same overspending window in both display modes.
    expect(expectedOverlay(monthly(92), true, NOW)?.ahead).toBe(true);
    expect(expectedOverlay(monthly(92), false, NOW)?.ahead).toBe(true);
    expect(expectedOverlay(monthly(60), true, NOW)?.ahead).toBe(false);
    expect(expectedOverlay(monthly(60), false, NOW)?.ahead).toBe(false);
  });

  it("shades the overspend band in both modes, spanning edge to marker", () => {
    // Used 92% against an expected 80%.
    const used = expectedOverlay(monthly(92), true, NOW);
    expect(used?.gap?.left).toBeCloseTo(80, 5);
    expect(used?.gap?.width).toBeCloseTo(12, 5);

    // Same window shown as remaining: edge at 8%, marker at 20%.
    const left = expectedOverlay(monthly(92), false, NOW);
    expect(left?.gap?.left).toBeCloseTo(8, 5);
    expect(left?.gap?.width).toBeCloseTo(12, 5);
  });

  it("draws no band when under budget", () => {
    expect(expectedOverlay(monthly(60), true, NOW)?.gap).toBeNull();
  });

  it("draws no band for a hairline difference", () => {
    // Half a percent off pace is not worth a stripe.
    expect(expectedOverlay(monthly(80.5), true, NOW)?.gap).toBeNull();
  });

  it("passes the silence through", () => {
    const session = win({
      windowMinutes: 300,
      resetsAt: new Date(NOW + 150 * MINUTE).toISOString(),
    });
    expect(expectedOverlay(session, true, NOW)).toBeNull();
  });
});
