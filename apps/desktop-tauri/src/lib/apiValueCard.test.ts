import { describe, expect, it } from "vitest";
import type { LocalApiValuePeriod, LocalApiValueProvider } from "../types/bridge";
import { buildApiValueCard, formatPeriodChange, ringSegments } from "./apiValueCard";

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
  periods: Partial<
    Record<"today" | "yesterday" | "thirtyDays" | "priorThirtyDays", LocalApiValuePeriod>
  >,
): LocalApiValueProvider {
  return {
    providerId,
    today: periods.today ?? empty,
    yesterday: periods.yesterday ?? empty,
    thirtyDays: periods.thirtyDays ?? empty,
    priorThirtyDays: periods.priorThirtyDays ?? empty,
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

  it("keeps an idle provider in the legend once another has data", () => {
    const model = buildApiValueCard(
      [
        provider("codex", { today: empty }),
        provider("claude", {
          today: period({ apiValueUsd: 31.88, tokens: 900, pricedTokens: 900, totalTokens: 900, hasData: true }),
        }),
      ],
      "today",
      "apiValue",
    );

    expect(model.isEmpty).toBe(false);
    expect(model.slices.map((slice) => slice.providerId)).toEqual(["claude", "codex"]);
    // The idle row carries a real zero, and the active one still owns the ring.
    expect(model.slices[1]).toMatchObject({ providerId: "codex", value: 0, share: 0 });
    expect(model.slices[0].share).toBe(1);
    expect(model.total).toBeCloseTo(31.88);
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

  it("reports dollar period-over-period for today vs yesterday", () => {
    const model = buildApiValueCard(
      [
        provider("codex", {
          today: period({
            apiValueUsd: 12,
            tokens: 1000,
            pricedTokens: 1000,
            totalTokens: 1000,
            hasData: true,
          }),
          yesterday: period({
            apiValueUsd: 10,
            tokens: 800,
            pricedTokens: 800,
            totalTokens: 800,
            hasData: true,
          }),
        }),
      ],
      "today",
      "apiValue",
    );
    expect(model.periodChange).toEqual({
      versusLabel: "vs yesterday",
      hasPrior: true,
      percent: 20,
    });
  });

  it("reports 30d vs prior 30d dollar change", () => {
    const model = buildApiValueCard(
      [
        provider("codex", {
          thirtyDays: period({
            apiValueUsd: 118,
            tokens: 1,
            pricedTokens: 1,
            totalTokens: 1,
            hasData: true,
          }),
          priorThirtyDays: period({
            apiValueUsd: 100,
            tokens: 1,
            pricedTokens: 1,
            totalTokens: 1,
            hasData: true,
          }),
        }),
      ],
      "thirtyDays",
      "apiValue",
    );
    expect(model.periodChange?.versusLabel).toBe("vs prior 30d");
    expect(model.periodChange?.percent).toBeCloseTo(18);
  });
});

describe("formatPeriodChange", () => {
  it("formats signed percent labels", () => {
    expect(
      formatPeriodChange({ versusLabel: "vs prior 30d", hasPrior: true, percent: 18.4 }),
    ).toBe("+18% vs prior 30d");
    expect(
      formatPeriodChange({ versusLabel: "vs yesterday", hasPrior: true, percent: -5 }),
    ).toBe("-5% vs yesterday");
    expect(
      formatPeriodChange({ versusLabel: "vs yesterday", hasPrior: true, percent: null }),
    ).toBe("New activity vs yesterday");
    expect(
      formatPeriodChange({ versusLabel: "vs yesterday", hasPrior: false, percent: null }),
    ).toBeNull();
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
