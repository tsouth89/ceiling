import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { QuotaHistoryChart } from "./QuotaHistoryChart";

describe("QuotaHistoryChart", () => {
  it("shows complete local times instead of truncating timestamps", () => {
    const { container } = render(
      <QuotaHistoryChart
        providerId="claude"
        animations={false}
        data={[
          {
            recordedAt: "2026-07-14T14:47:00Z",
            windows: [{ id: "session", label: "Session (5h)", usedPercent: 12 }],
          },
          {
            recordedAt: "2026-07-14T15:23:00Z",
            windows: [{ id: "session", label: "Session (5h)", usedPercent: 4 }],
          },
        ]}
      />,
    );

    const axisLabels = [...container.querySelectorAll(".chart__axis > span:not(.chart__axis-max)")]
      .map((node) => node.textContent ?? "");

    expect(axisLabels).toHaveLength(2);
    expect(axisLabels.every((label) => label.includes(":"))).toBe(true);
    expect(axisLabels).not.toContain("47 AM");
    expect(axisLabels).not.toContain("23 AM");
  });
});
