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
  floatBarInformationMode: "exact",
  floatBarContrast: "auto",
  floatBarShowCost: false,
  floatBarShowResetInline: false,
  floatBarDarkText: false,
  floatBarClickThrough: false,
  floatBarProviderIds: [],
  enabledProviders: ["codex", "claude", "cursor", "grok"],
  providerOrder: ["codex", "claude", "cursor", "grok"],
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

  it("lists enabled providers for the taskbar strip and can pin a custom order", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={set} />,
    );

    expect(screen.getByText("Providers on the strip")).toBeInTheDocument();
    expect(
      screen.getByRole("checkbox", { name: "Show Grok on taskbar strip" }),
    ).toBeChecked();

    fireEvent.click(
      screen.getByRole("button", { name: "Move Grok up" }),
    );
    expect(set).toHaveBeenCalledWith({
      floatBarProviderIds: ["codex", "claude", "grok", "cursor"],
    });
  });

  it("restores automatic strip order", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection
        settings={{
          ...settings,
          floatBarProviderIds: ["grok", "cursor"],
        }}
        saving={false}
        set={set}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Use automatic order" }));
    expect(set).toHaveBeenCalledWith({ floatBarProviderIds: [] });
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
