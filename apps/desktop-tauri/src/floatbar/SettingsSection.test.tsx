import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SettingsSnapshot } from "../types/bridge";
import FloatBarSettingsSection from "./SettingsSection";

vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

const settings = {
  floatBarEnabled: true,
  taskbarWidgetEnabled: true,
  taskbarWidgetAllMonitors: false,
  floatBarOpacity: 90,
  floatBarScale: 100,
  floatBarOrientation: "horizontal",
  floatBarStyle: "floating",
  taskbarWidgetOpenOnHover: true,
  floatBarDensity: "standard",
  floatBarContrast: "auto",
  floatBarShowCost: false,
  floatBarShowResetInline: false,
  floatBarDarkText: false,
  floatBarClickThrough: false,
} as unknown as SettingsSnapshot;

describe("FloatBar settings", () => {
  it("does not offer the legacy API-equivalent cost toggle", () => {
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={vi.fn()} />,
    );

    expect(screen.queryByText("FloatBarShowCost")).not.toBeInTheDocument();
  });

  it("shows independent taskbar and floating bar controls", () => {
    render(
      <FloatBarSettingsSection
        settings={settings}
        saving={false}
        set={vi.fn()}
      />,
    );

    expect(screen.getByText("Taskbar Usage")).toBeInTheDocument();
    expect(screen.getByText("Show Taskbar Usage")).toBeInTheDocument();
    expect(screen.getByText("Floating Bar")).toBeInTheDocument();
    expect(screen.getByText("Show Floating Bar")).toBeInTheDocument();
    expect(screen.getByText("Open on Hover")).toBeInTheDocument();
    expect(screen.getByText("Show on All Monitors")).toBeInTheDocument();
    expect(screen.getByText("Orientation")).toBeInTheDocument();
    expect(screen.getByText("Density")).toBeInTheDocument();
    expect(screen.queryByText("Placement")).not.toBeInTheDocument();
  });

  it("persists the taskbar hover preference", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection
        settings={settings}
        saving={false}
        set={set}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "Open on Hover" }));
    expect(set).toHaveBeenCalledWith({ taskbarWidgetOpenOnHover: false });
  });

  it("toggles the taskbar without changing the floating bar", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={set} />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "Show Taskbar Usage" }));
    expect(set).toHaveBeenCalledWith({ taskbarWidgetEnabled: false });
    expect(set).not.toHaveBeenCalledWith(expect.objectContaining({ floatBarEnabled: false }));
  });

  it("persists the all-monitors preference independently", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={set} />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "Show on All Monitors" }));
    expect(set).toHaveBeenCalledWith({ taskbarWidgetAllMonitors: true });
  });
});
