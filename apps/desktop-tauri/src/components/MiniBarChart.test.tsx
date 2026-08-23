import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SimpleBarChart, StackedBarChart } from "./MiniBarChart";

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
});
