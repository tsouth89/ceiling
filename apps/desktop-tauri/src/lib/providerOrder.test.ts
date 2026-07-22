import { describe, expect, it } from "vitest";
import { orderProviderSnapshots } from "./providerOrder";
import type { ProviderCatalogEntry, ProviderUsageSnapshot } from "../types/bridge";

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
  };
}

describe("orderProviderSnapshots", () => {
  it("uses persisted provider order before catalog order", () => {
    const ordered = orderProviderSnapshots(
      [
        snapshot("codex", "Codex"),
        snapshot("claude", "Claude"),
        snapshot("gemini", "Gemini"),
      ],
      catalog,
      ["codex", "claude", "gemini"],
      ["gemini", "claude", "codex"],
    );

    expect(ordered.map((provider) => provider.providerId)).toEqual([
      "gemini",
      "claude",
      "codex",
    ]);
  });

  it("excludes cached snapshots for providers the user stopped tracking", () => {
    const ordered = orderProviderSnapshots(
      [snapshot("codex", "Codex"), snapshot("gemini", "Gemini")],
      catalog,
      ["codex"],
      ["codex", "gemini"],
    );

    expect(ordered.map((provider) => provider.providerId)).toEqual(["codex"]);
  });
});

describe("orderProviderSnapshots with several accounts", () => {
  const snap = (providerId: string, accountId: string) =>
    ({
      providerId,
      accountId,
      displayName: providerId,
    }) as unknown as Parameters<typeof orderProviderSnapshots>[0][number];

  const catalog = [
    { id: "codex", displayName: "Codex" },
    { id: "claude", displayName: "Claude" },
  ] as unknown as Parameters<typeof orderProviderSnapshots>[1];

  it("keeps both accounts of a provider", () => {
    const out = orderProviderSnapshots(
      [snap("codex", "b"), snap("codex", "a")],
      catalog,
      ["codex"],
    );

    expect(out).toHaveLength(2);
  });

  it("orders two accounts the same way regardless of arrival order", () => {
    // Fetches finish in whatever order they finish. Without a tiebreak the
    // comparator was non-transitive and the rows swapped between refreshes.
    const one = orderProviderSnapshots(
      [snap("codex", "b"), snap("codex", "a")],
      catalog,
      ["codex"],
    ).map((p) => p.accountId);
    const other = orderProviderSnapshots(
      [snap("codex", "a"), snap("codex", "b")],
      catalog,
      ["codex"],
    ).map((p) => p.accountId);

    expect(one).toEqual(other);
  });

  it("keeps a provider's accounts adjacent", () => {
    const out = orderProviderSnapshots(
      [snap("codex", "b"), snap("claude", "c"), snap("codex", "a")],
      catalog,
      ["codex", "claude"],
    ).map((p) => p.providerId);

    // An unrelated provider must not be sorted in between two accounts.
    expect(out).toEqual(["codex", "codex", "claude"]);
  });
});
