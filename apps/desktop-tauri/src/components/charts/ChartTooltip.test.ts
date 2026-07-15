import { describe, expect, it } from "vitest";
import { clampChartTooltipLeft } from "./ChartTooltip";

describe("clampChartTooltipLeft", () => {
  it("centers a tooltip when there is room", () => {
    expect(clampChartTooltipLeft(140, 100, 280)).toBe(90);
  });

  it("keeps a tooltip inside the right edge", () => {
    expect(clampChartTooltipLeft(274, 120, 280)).toBe(152);
  });

  it("keeps a tooltip inside the left edge", () => {
    expect(clampChartTooltipLeft(2, 120, 280)).toBe(8);
  });
});
