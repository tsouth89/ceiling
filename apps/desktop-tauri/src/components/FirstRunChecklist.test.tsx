import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FirstRunChecklist } from "./FirstRunChecklist";

const t = (k: string) => k;

function setup(overrides: Partial<Parameters<typeof FirstRunChecklist>[0]> = {}) {
  const props = {
    enabledCount: 0,
    hasWorkingAuth: false,
    floatbarEnabled: false,
    onOpenProviders: vi.fn(),
    onOpenDisplay: vi.fn(),
    onDismiss: vi.fn(),
    t,
    ...overrides,
  };
  render(<FirstRunChecklist {...props} />);
  return props;
}

describe("FirstRunChecklist", () => {
  it("renders all three steps as actionable when nothing is set up", () => {
    setup();
    expect(screen.getByText("FirstRunStepEnable")).toBeInTheDocument();
    expect(screen.getByText("FirstRunStepAuth")).toBeInTheDocument();
    expect(screen.getByText("FirstRunStepFloatbar")).toBeInTheDocument();
    // Steps 1 and 2 both link to provider settings.
    expect(screen.getAllByText("FirstRunOpenProviders")).toHaveLength(2);
  });

  it("marks a completed step done and drops its action button", () => {
    setup({ enabledCount: 2, floatbarEnabled: true });
    const enable = screen.getByText("FirstRunStepEnable").closest("li");
    expect(enable?.className).toContain("first-run__step--done");
    // Only the auth step's action remains (enable + floatbar are done).
    expect(screen.getAllByText("FirstRunOpenProviders")).toHaveLength(1);
    expect(screen.queryByText("FirstRunOpenDisplay")).toBeNull();
  });

  it("dismiss calls onDismiss", () => {
    const props = setup();
    fireEvent.click(screen.getByText("FirstRunDismiss"));
    expect(props.onDismiss).toHaveBeenCalled();
  });

  it("the floatbar step opens display settings", () => {
    const props = setup();
    fireEvent.click(screen.getByText("FirstRunOpenDisplay"));
    expect(props.onOpenDisplay).toHaveBeenCalled();
  });
});
