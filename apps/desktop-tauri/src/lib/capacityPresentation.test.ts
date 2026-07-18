import { describe, expect, it } from "vitest";
import {
  capacityFreshness,
  constrainingWindow,
  glanceMeters,
  activePromoBoosts,
  activePromoInclusions,
  providerGlanceStatus,
  resetCreditsAvailable,
  codexResetCredits,
  calmPresentation,
} from "./capacityPresentation";
import type {
  PaceSnapshot,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";

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

  it("keeps Cursor plan as hero and shows reported Auto and API companions", () => {
    const meters = glanceMeters(
      provider({
        primary: window(62),
        primaryLabel: "Monthly",
        secondary: window(90),
        secondaryLabel: "Auto",
        extraRateWindows: [
          { id: "cursor-api", title: "API", window: window(12) },
        ],
      }),
    );
    expect(meters.primary.label).toBe("Monthly");
    expect(meters.primary.window.usedPercent).toBe(62);
    expect(meters.companions.map((meter) => meter.label)).toEqual(["Auto", "API"]);
    expect(meters.companions.map((meter) => meter.window.usedPercent)).toEqual([
      90, 12,
    ]);
  });

  it("keeps Cursor companion order stable when API is hotter than Auto", () => {
    const meters = glanceMeters(
      provider({
        primary: window(40),
        primaryLabel: "Monthly",
        secondary: window(55),
        secondaryLabel: "Auto",
        extraRateWindows: [
          { id: "cursor-api", title: "API", window: window(88) },
        ],
      }),
    );
    expect(meters.companions.map((meter) => meter.label)).toEqual(["Auto", "API"]);
    expect(meters.companions.map((meter) => meter.window.usedPercent)).toEqual([
      55, 88,
    ]);
  });

  it("keeps Claude weekly visible even when it is quieter than the session", () => {
    const meters = glanceMeters(
      provider({
        providerId: "claude",
        displayName: "Claude",
        primary: window(45),
        primaryLabel: "Session",
        secondary: window(20),
        secondaryLabel: "Weekly",
      }),
    );
    expect(meters.companions.map((meter) => meter.label)).toEqual(["Weekly"]);
    expect(meters.companions[0].window.usedPercent).toBe(20);
  });

  it("reports glance status from constraining pressure", () => {
    expect(providerGlanceStatus(provider({ error: "nope" }))).toBe("error");
    expect(
      providerGlanceStatus(
        provider({
          primary: window(10),
          secondary: window(95),
          secondaryLabel: "Auto",
        }),
      ),
    ).toBe("warning");
    expect(
      providerGlanceStatus(
        provider({
          primary: window(100),
        }),
      ),
    ).toBe("exhausted");
  });

  it("reports freshness precedence error > stale > live", () => {
    expect(capacityFreshness(provider({ error: "fail" }))).toBe("error");
    expect(
      capacityFreshness(
        provider({
          updatedAt: new Date(Date.now() - 20 * 60 * 1000).toISOString(),
        }),
      ),
    ).toBe("stale");
    expect(capacityFreshness(provider())).toBe("live");
  });

  it("keeps live freshness when only some windows are inactive (SOU-152)", () => {
    // Inactive windows are surfaced as their own rows, never as a
    // provider-level "lifted" freshness state.
    const inactiveRateWindows = [
      {
        id: "cursor-auto",
        title: "Auto",
        description: "Not currently enforced by Cursor",
      },
    ];
    expect(capacityFreshness(provider({ inactiveRateWindows }))).toBe("live");
    // A stale timestamp still wins over inactive windows.
    expect(
      capacityFreshness(
        provider({
          updatedAt: new Date(Date.now() - 20 * 60 * 1000).toISOString(),
          inactiveRateWindows,
        }),
      ),
    ).toBe("stale");
  });

  it("separates boost promos from inclusion notes", () => {
    const snap = provider({
      promoSignals: [
        {
          id: "claude-weekly-promo",
          kind: "boost",
          title: "Weekly promo",
          description: "Temporary promotional weekly capacity",
        },
        {
          id: "cursor-grok",
          kind: "inclusion",
          title: "Grok in Auto",
          description: "Model included in Auto pool",
        },
      ],
    });
    expect(activePromoBoosts(snap).map((p) => p.id)).toEqual([
      "claude-weekly-promo",
    ]);
    expect(activePromoInclusions(snap).map((p) => p.id)).toEqual(["cursor-grok"]);
  });

  it("reads reset availability without treating it as a usage meter", () => {
    const snap = provider({
      resetCreditsAvailable: 1,
      extraRateWindows: [
        {
          id: "reset-credits",
          title: "Reset credits",
          window: { ...window(0), resetDescription: "1 reset credit available" },
        },
      ],
    });
    expect(resetCreditsAvailable(snap)).toBe(1);
    expect(glanceMeters(snap).companions).toEqual([]);
  });

  it("reports a known zero banked-reset count instead of hiding it", () => {
    expect(resetCreditsAvailable(provider({ resetCreditsAvailable: 0 }))).toBe(0);
  });

  it("exposes Codex banked resets only for Codex, including the zero state", () => {
    // Codex is the only provider with banked resets, so the persistent
    // indicator stays Codex-scoped and shows even when the count is zero.
    expect(
      codexResetCredits(provider({ providerId: "codex", resetCreditsAvailable: 0 })),
    ).toBe(0);
    expect(
      codexResetCredits(provider({ providerId: "codex", resetCreditsAvailable: 3 })),
    ).toBe(3);
    // Another provider reporting the field must never light up the Codex chip.
    expect(
      codexResetCredits(provider({ providerId: "cursor", resetCreditsAvailable: 2 })),
    ).toBeNull();
  });

  describe("calmPresentation", () => {
    const pace = (over: Partial<PaceSnapshot> = {}): PaceSnapshot => ({
      windowLabel: "Weekly",
      stage: "on_track",
      deltaPercent: 0,
      willLastToReset: true,
      etaSeconds: null,
      expectedUsedPercent: 40,
      actualUsedPercent: 40,
      ...over,
    });
    const withReset = (usedPercent: number): RateWindowSnapshot => ({
      ...window(usedPercent),
      resetsAt: new Date(Date.now() + 3_600_000).toISOString(),
    });

    it("shows a steady pace state when fresh pace lasts to reset", () => {
      const snap = provider({ pace: pace({ willLastToReset: true }) });
      const result = calmPresentation(snap, constrainingWindow(snap));
      expect(result.pace).toEqual({ label: "On pace", tone: "steady" });
      expect(result.showExactFallback).toBe(false);
    });

    it("warns only when there is a real finite ETA to run out", () => {
      const risky = provider({
        pace: pace({ willLastToReset: false, etaSeconds: 3600 }),
      });
      expect(calmPresentation(risky, constrainingWindow(risky)).pace).toEqual({
        label: "Running low",
        tone: "watch",
      });

      // No usable ETA: stay silent rather than invent a state.
      const vague = provider({
        pace: pace({ willLastToReset: false, etaSeconds: null }),
      });
      expect(calmPresentation(vague, constrainingWindow(vague)).pace).toBeNull();
    });

    it("never invents a pace state when pace is missing, stale, or errored", () => {
      const noPace = provider({ pace: null });
      expect(calmPresentation(noPace, constrainingWindow(noPace)).pace).toBeNull();

      const stale = provider({
        pace: pace({ willLastToReset: true }),
        updatedAt: new Date(Date.now() - 60 * 60 * 1000).toISOString(),
      });
      expect(calmPresentation(stale, constrainingWindow(stale)).pace).toBeNull();

      const errored = provider({ pace: pace({ willLastToReset: true }), error: "boom" });
      expect(calmPresentation(errored, constrainingWindow(errored)).pace).toBeNull();
    });

    it("falls back to exact percentage only when neither pace nor a reset exists", () => {
      const noReset = provider({ pace: null, primary: window(30) });
      const bare = calmPresentation(noReset, constrainingWindow(noReset));
      expect(bare.hasReset).toBe(false);
      expect(bare.showExactFallback).toBe(true);

      const withResetSnap = provider({ pace: null, primary: withReset(30) });
      const reset = calmPresentation(withResetSnap, constrainingWindow(withResetSnap));
      expect(reset.hasReset).toBe(true);
      expect(reset.showExactFallback).toBe(false);
    });
  });
});
