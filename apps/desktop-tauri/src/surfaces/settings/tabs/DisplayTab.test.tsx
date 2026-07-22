import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key, language: "english" }),
}));
// The FloatBar section pulls in its own bridge dependencies; it is irrelevant
// to these display-preference tests.
vi.mock("../../../floatbar", () => ({
  FloatBarSettingsSection: () => null,
}));

import DisplayTab from "./DisplayTab";
import type { SettingsSnapshot } from "../../../types/bridge";

const baseSettings = {
  switcherShowsIcons: false,
  menuBarShowsHighestUsage: false,
  menuBarShowsPercent: false,
  menuBarDisplayMode: "detailed",
  windowScalePercent: 100,
  showAsUsed: false,
  showAllTokenAccountsInMenu: false,
  resetTimeRelative: false,
  showResetWhenExhausted: false,
} as unknown as SettingsSnapshot;

function renderTab(set: (patch: Record<string, unknown>) => void) {
  return render(
    <DisplayTab settings={baseSettings} set={set as never} saving={false} />,
  );
}

describe("DisplayTab", () => {
  it("does not expose the unused token-account display setting", () => {
    renderTab(vi.fn());
    expect(screen.queryByText("ShowAllTokenAccountsLabel")).not.toBeInTheDocument();
  });

  it("keeps legacy dashboard scaling out of the primary settings UI", () => {
    renderTab(vi.fn());
    expect(
      screen.queryByRole("slider", { name: "WindowScaleAriaLabel" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("WindowScaleLabel")).not.toBeInTheDocument();
  });

  it("updates the exhausted reset display preference", () => {
    const set = vi.fn();
    renderTab(set);

    fireEvent.click(screen.getByRole("checkbox", { name: "ShowResetWhenExhausted" }));

    expect(set).toHaveBeenCalledWith({ showResetWhenExhausted: true });
  });
});
