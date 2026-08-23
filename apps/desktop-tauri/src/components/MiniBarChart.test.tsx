import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SimpleBarChart, StackedBarChart } from "./MiniBarChart";
import { expectNoAccessibilityViolations } from "../test/accessibility";

vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({ t: () => "No data" }),
}));

describe("MiniBarChart empty state", () => {
  it("names an empty simple chart from its label", () => {
    render(<SimpleBarChart points={[]} label="Daily cost" />);

    expect(screen.getByRole("img", { name: "Daily cost: No data" })).toBeTruthy();
  });

  it("gives an unlabeled empty stacked chart a localized name", () => {
    render(<StackedBarChart points={[]} />);

    expect(screen.getByRole("img", { name: "No data" })).toBeTruthy();
  });

  it("has no detectable accessibility violations", async () => {
    const { container } = render(
      <SimpleBarChart
        label="Daily cost"
        points={[
          { date: "2026-08-18", value: 3 },
          { date: "2026-08-19", value: 5 },
        ]}
      />,
    );

    await expectNoAccessibilityViolations(container);
  });
});
