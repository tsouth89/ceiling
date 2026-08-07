import { describe, expect, it } from "vitest";

import { formatEta } from "./PaceSection";

describe("formatEta", () => {
  it("formats zero as minutes", () => {
    expect(formatEta(0)).toBe("0m");
  });

  it("clamps negative input at zero minutes", () => {
    expect(formatEta(-30)).toBe("0m");
  });

  it("rounds seconds to minutes", () => {
    expect(formatEta(59)).toBe("1m");
    expect(formatEta(60)).toBe("1m");
  });

  it("keeps sub-hour values as minutes", () => {
    expect(formatEta(59 * 60)).toBe("59m");
  });

  it("converts an hour boundary to hours", () => {
    expect(formatEta(60 * 60)).toBe("1h");
  });

  it("converts 23h59m to a day under current rounding", () => {
    expect(formatEta(23 * 3600 + 59 * 60)).toBe("1d");
  });

  it("converts a full day to days", () => {
    expect(formatEta(24 * 3600)).toBe("1d");
    expect(formatEta(48 * 3600)).toBe("2d");
  });
});
