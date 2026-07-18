import { describe, expect, it } from "vitest";
import type { LocalApiValuePeriod, LocalApiValueProvider } from "../types/bridge";
import { buildApiValueCard, ringSegments } from "./apiValueCard";

function period(over: Partial<LocalApiValuePeriod>): LocalApiValuePeriod {
  return {
    apiValueUsd: 0,
    tokens: 0,
    pricedTokens: 0,
    totalTokens: 0,
    hasData: false,
    ...over,
  };
}

const empty = period({});

function provider(
  providerId: string,
  periods: Partial<Record<"today" | "yesterday" | "thirtyDays", LocalApiValuePeriod>>,
): LocalApiValueProvider {
  return {
    providerId,
    today: periods.today ?? empty,
    yesterday: periods.yesterday ?? empty,
    thirtyDays: periods.thirtyDays ?? empty,
  };
}

describe("buildApiValueCard", () => {
  it("returns an empty model when no provider has data", () => {
    const model = buildApiValueCard([provider("codex", {})], "today", "apiValue");
    expect(model.isEmpty).toBe(true);
    expect(model.slices).toHaveLength(0);
    expect(model.total).toBe(0);
    expect(model.coverage).toBeNull();
  });

  it("ranks a single provider at 100% share", () => {
    const model = buildApiValueCard(
      [
        provider("codex", {
          today: period({ apiValueUsd: 12, tokens: 1000, pricedTokens: 1000, totalTokens: 1000, hasData: true }),
        }),
      ],
      "today",
      "apiValue",
    );
    expect(model.isEmpty).toBe(false);
    expect(model.slices).toEqual([{ providerId: "codex", value: 12, share: 1 }]);
    expect(model.total).toBe(12);
    expect(model.coverage).toBe(1);
    expect(model.unpricedProviderIds).toHaveLength(0);
  });

  it("ranks multiple providers by value with shares summing to 1", () => {
    const model = buildApiValueCard(
      [
        provider("claude", {
          thirtyDays: period({ apiValueUsd: 30, tokens: 3000, pricedTokens: 3000, totalTokens: 3000, hasData: true }),
        }),
        provider("codex", {
          thirtyDays: period({ apiValueUsd: 90, tokens: 9000, pricedTokens: 9000, totalTokens: 9000, hasData: true }),
        }),
      ],
      "thirtyDays",
      "apiValue",
    );
    expect(model.slices.map((s) => s.providerId)).toEqual(["codex", "claude"]);
    expect(model.total).toBe(120);
    expect(model.slices[0].share).toBeCloseTo(0.75);
    expect(model.slices[1].share).toBeCloseTo(0.25);
    expect(model.slices.reduce((sum, s) => sum + s.share, 0)).toBeCloseTo(1);
  });

  it("keeps a tiny share as a nonzero slice", () => {
    const model = buildApiValueCard(
      [
        provider("codex", {
          today: period({ apiValueUsd: 999.99, tokens: 1_000_000, pricedTokens: 1_000_000, totalTokens: 1_000_000, hasData: true }),
        }),
        provider("claude", {
          today: period({ apiValueUsd: 0.01, tokens: 10, pricedTokens: 10, totalTokens: 10, hasData: true }),
        }),
      ],
      "today",
      "apiValue",
    );
    const tiny = model.slices.find((s) => s.providerId === "claude")!;
    expect(tiny.value).toBe(0.01);
    expect(tiny.share).toBeGreaterThan(0);
  });

  it("omits providers without data this period rather than counting zero", () => {
    const model = buildApiValueCard(
      [
        provider("codex", {
          today: period({ apiValueUsd: 5, tokens: 500, pricedTokens: 500, totalTokens: 500, hasData: true }),
          yesterday: empty, // no data yesterday
        }),
      ],
      "yesterday",
      "apiValue",
    );
    expect(model.isEmpty).toBe(true);
    expect(model.slices).toHaveLength(0);
  });

  it("reports partial pricing coverage and the affected providers", () => {
    const model = buildApiValueCard(
      [
        provider("codex", {
          thirtyDays: period({ apiValueUsd: 8, tokens: 1000, pricedTokens: 750, totalTokens: 1000, hasData: true }),
        }),
        provider("claude", {
          thirtyDays: period({ apiValueUsd: 4, tokens: 1000, pricedTokens: 1000, totalTokens: 1000, hasData: true }),
        }),
      ],
      "thirtyDays",
      "apiValue",
    );
    // 1750 priced of 2000 total = 87.5%.
    expect(model.coverage).toBeCloseTo(0.875);
    expect(model.unpricedProviderIds).toEqual(["codex"]);
  });

  it("keeps coverage on the tokens metric too (all-unpriced period)", () => {
    const model = buildApiValueCard(
      [
        provider("codex", {
          today: period({ apiValueUsd: 0, tokens: 1000, pricedTokens: 0, totalTokens: 1000, hasData: true }),
        }),
      ],
      "today",
      "tokens",
    );
    // Not empty (there is token data), but zero dollars and zero coverage.
    expect(model.isEmpty).toBe(false);
    expect(model.total).toBe(1000);
    expect(model.coverage).toBe(0);
    expect(model.unpricedProviderIds).toEqual(["codex"]);
  });
});

describe("ringSegments", () => {
  it("lays segments end to end with dash = share x circumference", () => {
    const c = 100;
    const segments = ringSegments(
      [
        { providerId: "codex", value: 75, share: 0.75 },
        { providerId: "claude", value: 25, share: 0.25 },
      ],
      c,
    );
    expect(segments[0]).toEqual({ providerId: "codex", dash: 75, offset: -0 });
    expect(segments[1]).toEqual({ providerId: "claude", dash: 25, offset: -75 });
  });
});
