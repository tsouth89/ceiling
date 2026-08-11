import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import { useTabListKeyboard, type TabListActivation } from "./useTabListKeyboard";

const TABS = ["alpha", "beta", "gamma"] as const;
type Tab = (typeof TABS)[number];

function Harness({
  activation,
  onSelect,
  initial = "alpha",
  disabled = [],
}: {
  activation?: TabListActivation;
  onSelect?: (id: Tab) => void;
  initial?: Tab;
  disabled?: Tab[];
}) {
  const [selected, setSelected] = useState<Tab>(initial);
  const { tabListProps, getTabProps, getPanelProps } = useTabListKeyboard<Tab>({
    tabIds: TABS,
    selectedId: selected,
    onSelect: (id) => {
      setSelected(id);
      onSelect?.(id);
    },
    activation,
    isTabDisabled: (id) => disabled.includes(id),
  });

  return (
    <div>
      <div {...tabListProps} aria-label="Sections">
        {TABS.map((id) => (
          <button
            key={id}
            type="button"
            {...getTabProps(id)}
            disabled={disabled.includes(id)}
            onClick={() => {
              setSelected(id);
              onSelect?.(id);
            }}
          >
            {id}
          </button>
        ))}
      </div>
      <div {...getPanelProps()}>{selected} panel</div>
    </div>
  );
}

const tab = (name: Tab) => screen.getByRole("tab", { name });

describe("useTabListKeyboard", () => {
  it("gives the strip a single tab stop on the selected tab", () => {
    render(<Harness initial="beta" />);
    expect(tab("alpha").tabIndex).toBe(-1);
    expect(tab("beta").tabIndex).toBe(0);
    expect(tab("gamma").tabIndex).toBe(-1);
  });

  it("keeps the strip reachable when nothing is selected yet", () => {
    function Unselected() {
      const { tabListProps, getTabProps } = useTabListKeyboard<Tab>({
        tabIds: TABS,
        selectedId: null,
        onSelect: () => {},
      });
      return (
        <div {...tabListProps}>
          {TABS.map((id) => (
            <button key={id} type="button" {...getTabProps(id)}>
              {id}
            </button>
          ))}
        </div>
      );
    }
    render(<Unselected />);
    expect(tab("alpha").tabIndex).toBe(0);
    expect(tab("alpha").getAttribute("aria-selected")).toBe("false");
  });

  it("moves to the next tab on ArrowRight and selects it automatically", () => {
    const onSelect = vi.fn();
    render(<Harness onSelect={onSelect} />);
    fireEvent.keyDown(tab("alpha"), { key: "ArrowRight" });
    expect(onSelect).toHaveBeenCalledWith("beta");
    expect(tab("beta")).toBe(document.activeElement);
    expect(tab("beta").getAttribute("aria-selected")).toBe("true");
  });

  it("wraps at both ends", () => {
    const onSelect = vi.fn();
    render(<Harness onSelect={onSelect} />);
    fireEvent.keyDown(tab("alpha"), { key: "ArrowLeft" });
    expect(onSelect).toHaveBeenLastCalledWith("gamma");
    fireEvent.keyDown(tab("gamma"), { key: "ArrowRight" });
    expect(onSelect).toHaveBeenLastCalledWith("alpha");
  });

  it("jumps to the ends with Home and End", () => {
    const onSelect = vi.fn();
    render(<Harness initial="beta" onSelect={onSelect} />);
    fireEvent.keyDown(tab("beta"), { key: "End" });
    expect(onSelect).toHaveBeenLastCalledWith("gamma");
    fireEvent.keyDown(tab("gamma"), { key: "Home" });
    expect(onSelect).toHaveBeenLastCalledWith("alpha");
  });

  it("ignores keys that are not strip movement", () => {
    const onSelect = vi.fn();
    render(<Harness onSelect={onSelect} />);
    fireEvent.keyDown(tab("alpha"), { key: "ArrowDown" });
    fireEvent.keyDown(tab("alpha"), { key: "a" });
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("skips disabled tabs", () => {
    const onSelect = vi.fn();
    render(<Harness onSelect={onSelect} disabled={["beta"]} />);
    fireEvent.keyDown(tab("alpha"), { key: "ArrowRight" });
    expect(onSelect).toHaveBeenCalledWith("gamma");
  });

  it("moves focus without selecting under manual activation", () => {
    const onSelect = vi.fn();
    render(<Harness activation="manual" onSelect={onSelect} />);
    fireEvent.keyDown(tab("alpha"), { key: "ArrowRight" });
    expect(onSelect).not.toHaveBeenCalled();
    expect(tab("beta")).toBe(document.activeElement);
    expect(tab("alpha").getAttribute("aria-selected")).toBe("true");

    // The tabs are real buttons, so Enter commits through the click handler.
    fireEvent.click(tab("beta"));
    expect(onSelect).toHaveBeenCalledWith("beta");
  });

  it("moves the tab stop to the focused tab, not the selected one", () => {
    // Under manual activation the two differ. If the stop stayed on the
    // selection, the strip would offer a second stop and Tab-back would land
    // somewhere the user did not leave.
    render(<Harness activation="manual" />);
    fireEvent.keyDown(tab("alpha"), { key: "ArrowRight" });
    expect(tab("beta").tabIndex).toBe(0);
    expect(tab("alpha").tabIndex).toBe(-1);
    expect(tab("alpha").getAttribute("aria-selected")).toBe("true");
  });

  it("does not re-select when the key cannot move anywhere", () => {
    // Settings sends an IPC surface-mode message from its select handler, so a
    // no-op Home is not free.
    const onSelect = vi.fn();
    render(<Harness onSelect={onSelect} />);
    fireEvent.keyDown(tab("alpha"), { key: "Home" });
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("keeps the tab stop off a disabled selected tab", () => {
    // A disabled button cannot take focus, so leaving the stop there would drop
    // the whole strip out of the tab order.
    render(<Harness initial="alpha" disabled={["alpha"]} />);
    expect(tab("alpha").tabIndex).toBe(-1);
    expect(tab("beta").tabIndex).toBe(0);
  });

  it("pairs the selected tab with its panel and leaves the others unpaired", () => {
    render(<Harness initial="beta" />);
    const panel = screen.getByRole("tabpanel");
    expect(tab("beta").getAttribute("aria-controls")).toBe(panel.id);
    expect(panel.getAttribute("aria-labelledby")).toBe(tab("beta").id);
    expect(tab("alpha").getAttribute("aria-controls")).toBeNull();
    expect(tab("gamma").getAttribute("aria-controls")).toBeNull();
  });

  it("makes the panel focusable so a keyboard user can reach and scroll it", () => {
    render(<Harness />);
    expect(screen.getByRole("tabpanel").tabIndex).toBe(0);
  });

  it("names no tab from the panel when nothing is selected", () => {
    // A dangling aria-labelledby is worse than an unnamed panel: it promises a
    // label that assistive tech cannot resolve.
    function Unselected() {
      const { getPanelProps } = useTabListKeyboard<Tab>({
        tabIds: TABS,
        selectedId: null,
        onSelect: () => {},
      });
      return <div {...getPanelProps()}>panel</div>;
    }
    render(<Unselected />);
    const panel = screen.getByRole("tabpanel");
    expect(panel.getAttribute("aria-labelledby")).toBeNull();
    expect(panel.getAttribute("id")).toBeNull();
  });

  it("follows the selection so the panel always names the current tab", () => {
    render(<Harness />);
    fireEvent.keyDown(tab("alpha"), { key: "ArrowRight" });
    const panel = screen.getByRole("tabpanel");
    expect(panel.getAttribute("aria-labelledby")).toBe(tab("beta").id);
    expect(tab("beta").getAttribute("aria-controls")).toBe(panel.id);
  });
});
