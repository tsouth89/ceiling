import { beforeEach, describe, expect, it } from "vitest";

import {
  getIgnoredDetectedProviderIds,
  setDetectedProviderIgnored,
} from "./detectedProviderPreferences";
import {
  hydrateProviderSlots,
  orderedEnabledProviderSlots,
  providerPlaceholder,
} from "./trayProviders";
import type {
  ProviderCatalogEntry,
  ProviderUsageSnapshot,
} from "../types/bridge";

const catalog: ProviderCatalogEntry[] = [
  { id: "codex", displayName: "Codex", cookieDomain: null },
  { id: "claude", displayName: "Claude", cookieDomain: null },
  { id: "gemini", displayName: "Gemini", cookieDomain: null },
];

function snapshot(providerId: string, displayName: string): ProviderUsageSnapshot {
  return {
    providerId,
    displayName,
    primary: {
      usedPercent: 0,
      remainingPercent: 100,
      windowMinutes: null,
      resetsAt: null,
      resetDescription: null,
      isExhausted: false,
      reservePercent: null,
      reserveDescription: null,
    },
    primaryLabel: "Usage",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "test",
    updatedAt: "2026-01-01T00:00:00Z",
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
  };
}

describe("orderedEnabledProviderSlots", () => {
  it("honors providerOrder when non-empty", () => {
    const slots = orderedEnabledProviderSlots(
      catalog,
      ["codex", "claude", "gemini"],
      [],
      ["gemini", "claude", "codex"],
    );
    expect(slots.map((slot) => slot.id)).toEqual([
      "gemini",
      "claude",
      "codex",
    ]);
  });

  it("falls back to catalog order when providerOrder is empty", () => {
    const slots = orderedEnabledProviderSlots(
      catalog,
      ["claude", "codex"],
      [],
    );
    expect(slots.map((slot) => slot.id)).toEqual(["codex", "claude"]);
  });

  it("skips ids that are not enabled", () => {
    const slots = orderedEnabledProviderSlots(
      catalog,
      ["claude"],
      [],
      ["gemini", "claude"],
    );
    expect(slots.map((slot) => slot.id)).toEqual(["claude"]);
  });

  it("appends enabled ids missing from the ordered list without duplicates", () => {
    const slots = orderedEnabledProviderSlots(
      catalog,
      ["gemini", "claude"],
      [],
      ["gemini"],
    );
    expect(slots.map((slot) => slot.id)).toEqual(["gemini", "claude"]);
  });

  it("resolves display names through catalog, snapshot, then raw id", () => {
    const slots = orderedEnabledProviderSlots(
      catalog,
      ["codex", "custom", "gemini"],
      [snapshot("custom", "Custom Snapshot")],
      ["codex"],
    );
    expect(slots[0]).toEqual({ id: "codex", displayName: "Codex" });
    expect(slots[1]).toEqual({ id: "custom", displayName: "Custom Snapshot" });
    expect(slots[2]).toEqual({ id: "gemini", displayName: "Gemini" });
  });
});

describe("hydrateProviderSlots and providerPlaceholder", () => {
  it("hydrates from the provider map", () => {
    const providers = new Map([["codex", snapshot("codex", "Codex")]]);
    const hydrated = hydrateProviderSlots(
      [{ id: "codex", displayName: "Codex" }],
      providers,
    );
    expect(hydrated[0].providerId).toBe("codex");
    expect(hydrated[0].sourceLabel).toBe("test");
  });

  it("falls back to a placeholder for missing providers", () => {
    const hydrated = hydrateProviderSlots(
      [{ id: "missing", displayName: "Missing" }],
      new Map(),
    );
    expect(hydrated[0].providerId).toBe("missing");
    expect(hydrated[0].sourceLabel).toBe("pending");
    expect(hydrated[0].error).toContain("Loading");
  });

  it("providerPlaceholder returns a pending snapshot", () => {
    const placeholder = providerPlaceholder("codex", "Codex");
    expect(placeholder.providerId).toBe("codex");
    expect(placeholder.sourceLabel).toBe("pending");
    expect(placeholder.error).toContain("Loading provider data");
  });
});

describe("detectedProviderPreferences", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("starts with no ignored providers", () => {
    expect(getIgnoredDetectedProviderIds().size).toBe(0);
  });

  it("tracks ignored providers", () => {
    setDetectedProviderIgnored("codex", true);
    expect(getIgnoredDetectedProviderIds()).toContain("codex");
    setDetectedProviderIgnored("codex", false);
    expect(getIgnoredDetectedProviderIds().has("codex")).toBe(false);
  });
});
