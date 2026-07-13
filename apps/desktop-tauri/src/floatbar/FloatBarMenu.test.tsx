import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import FloatBarMenu from "./FloatBarMenu";

function setup(overrides: Partial<Parameters<typeof FloatBarMenu>[0]> = {}) {
  const props = {
    locked: false,
    clickThrough: false,
    onToggleLock: vi.fn(),
    onToggleClickThrough: vi.fn(),
    onOpenSettings: vi.fn(),
    onHide: vi.fn(),
    ...overrides,
  };
  const utils = render(<FloatBarMenu {...props} />);
  return { props, ...utils };
}

describe("FloatBarMenu", () => {
  it("renders the four purposeful actions", () => {
    const { getByText } = setup();
    for (const label of ["Lock", "Click-through", "Settings", "Hide"]) {
      expect(getByText(label)).toBeTruthy();
    }
  });

  it("fires the matching handler for each action", () => {
    const { props, getByText } = setup();
    fireEvent.click(getByText("Lock"));
    fireEvent.click(getByText("Click-through"));
    fireEvent.click(getByText("Settings"));
    fireEvent.click(getByText("Hide"));
    expect(props.onToggleLock).toHaveBeenCalledOnce();
    expect(props.onToggleClickThrough).toHaveBeenCalledOnce();
    expect(props.onOpenSettings).toHaveBeenCalledOnce();
    expect(props.onHide).toHaveBeenCalledOnce();
  });

  it("reflects lock + click-through active state", () => {
    const { getByText, container } = setup({ locked: true, clickThrough: true });
    // Locked flips the label to Unlock.
    expect(getByText("Unlock")).toBeTruthy();
    expect(
      container.querySelectorAll(".floatbar__menu-item--active"),
    ).toHaveLength(2);
    expect(
      container.querySelectorAll('[aria-checked="true"]'),
    ).toHaveLength(2);
  });
});
