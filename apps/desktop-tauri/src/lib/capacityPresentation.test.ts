import { describe, expect, it } from "vitest";
import {
  capacityFreshness,
  constrainingWindow,
} from "./capacityPresentation";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";

function window(usedPercent: number): RateWindowSnapshot {
  return {
    usedPercent,
    remainingPercent: 100 - usedPercent,
    windowMinutes: null,
    resetsAt: null,
    resetDescription: null,
    isExhausted: usedPercent >= 100,
    reservePercent: null,
    reserveDescription: null,
    reserveWillLastToReset: false,
    reserveEtaSeconds: null,
  };
}

function provider(
  overrides: Partial<ProviderUsageSnapshot> = {},
): ProviderUsageSnapshot {
  return {
    providerId: "cursor",
    displayName: "Cursor",
    primary: window(30),
    primaryLabel: "Monthly",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    inactiveRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "web",
    updatedAt: new Date().toISOString(),
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
    ...overrides,
  };
}

describe("capacityPresentation", () => {
  it("selects the highest used measured window as constraining", () => {
    const snap = provider({
      secondary: window(55),
      secondaryLabel: "Auto",
      extraRateWindows: [
        { id: "cursor-api", title: "API", window: window(10) },
      ],
    });
    const constraining = constrainingWindow(snap);
    expect(constraining.id).toBe("secondary");
    expect(constraining.label).toBe("Auto");
    expect(constraining.window.usedPercent).toBe(55);
  });

  it("reports freshness precedence error > stale > lifted > live", () => {
    expect(capacityFreshness(provider({ error: "fail" }))).toBe("error");
    expect(
      capacityFreshness(
        provider({
          updatedAt: new Date(Date.now() - 20 * 60 * 1000).toISOString(),
        }),
      ),
    ).toBe("stale");
    expect(
      capacityFreshness(
        provider({
          inactiveRateWindows: [
            {
              id: "cursor-auto",
              title: "Auto",
              description: "Not currently enforced by Cursor",
            },
          ],
        }),
      ),
    ).toBe("lifted");
    expect(capacityFreshness(provider())).toBe("live");
  });
});
