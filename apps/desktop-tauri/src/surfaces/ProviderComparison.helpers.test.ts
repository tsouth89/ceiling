import { describe, expect, it } from "vitest";

import type {
  LocalTokenBreakdown,
  LocalUsageComparisonPeriod,
} from "../types/bridge";
import {
  cacheShare,
  comparisonSummary,
  formatTokens,
  periodChange,
} from "./ProviderComparison";

describe("formatTokens", () => {
  it("formats a small value with compact notation", () => {
    expect(formatTokens(1_234)).toBe("1.2K");
  });

  it("formats a large value with compact notation", () => {
    expect(formatTokens(1_234_567)).toBe("1.2M");
  });
});

describe("cacheShare", () => {
  it("returns zero when processedTokens is not positive", () => {
    expect(cacheShare({ processedTokens: 0 } as LocalTokenBreakdown)).toBe(0);
    expect(cacheShare({ processedTokens: -10 } as LocalTokenBreakdown)).toBe(0);
  });

  it("returns cache tokens as a percentage of processed tokens", () => {
    const breakdown: LocalTokenBreakdown = {
      processedTokens: 200,
      cacheReadTokens: 30,
      cacheWriteTokens: 10,
    } as LocalTokenBreakdown;
    expect(cacheShare(breakdown)).toBe(20);
  });
});

describe("periodChange", () => {
  it("returns No change when both values are zero", () => {
    expect(
      periodChange({ previousTokens: 0, currentTokens: 0 } as LocalUsageComparisonPeriod),
    ).toBe("No change");
  });

  it("returns New activity when previous is zero and current is positive", () => {
    expect(
      periodChange({ previousTokens: 0, currentTokens: 10 } as LocalUsageComparisonPeriod),
    ).toBe("New activity");
  });

  it("returns a signed percentage change", () => {
    expect(
      periodChange({ previousTokens: 100, currentTokens: 150 } as LocalUsageComparisonPeriod),
    ).toBe("+50% vs prior");
    expect(
      periodChange({ previousTokens: 100, currentTokens: 50 } as LocalUsageComparisonPeriod),
    ).toBe("-50% vs prior");
  });
});

describe("comparisonSummary", () => {
  it("reports even activity", () => {
    expect(comparisonSummary("A", 100, "B", 100)).toBe(
      "Even activity across both providers",
    );
  });

  it("reports a leader that recorded all local activity", () => {
    expect(comparisonSummary("A", 0, "B", 50)).toBe(
      "B recorded all local activity",
    );
  });

  it("reports the Nx leader path", () => {
    expect(comparisonSummary("A", 100, "B", 50)).toBe(
      "A processed 2.0× more",
    );
  });
});
