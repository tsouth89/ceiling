import { describe, expect, it } from "vitest";
import { chartTooltipPosition } from "./chartTooltip";

const rect = { left: 100, top: 50, width: 280 };

describe("chartTooltipPosition", () => {
  it("anchors tooltips inward at both chart edges", () => {
    expect(chartTooltipPosition(102, 75, rect)).toEqual({ x: 4, y: 25, alignment: "start" });
    expect(chartTooltipPosition(379, 75, rect)).toEqual({ x: 276, y: 25, alignment: "end" });
  });

  it("centers tooltips away from the edges", () => {
    expect(chartTooltipPosition(240, 75, rect)).toEqual({ x: 140, y: 25, alignment: "center" });
  });
});
