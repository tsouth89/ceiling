import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  validateGlobalShortcut: vi.fn(),
}));

vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

vi.mock("../../../lib/tauri", () => tauriMocks);

import AdvancedTab from "./AdvancedTab";
import type { SettingsSnapshot } from "../../../types/bridge";

const settings = {
  globalShortcut: "Ctrl+Shift+U",
  taskbarToggleShortcut: "Ctrl+Shift+H",
  codexCustomSessionsDirs: [],
  agentSessionSshHosts: [],
  activeAgentSessionsEnabled: false,
  hidePersonalInfo: false,
  disableKeychainAccess: false,
  claudeAvoidKeychainPrompts: false,
  powertoysStatusPipeEnabled: false,
} as unknown as SettingsSnapshot;

describe("AdvancedTab shortcut registration", () => {
  beforeEach(() => vi.clearAllMocks());

  it("validates and saves the dashboard shortcut", async () => {
    const set = vi.fn();
    render(<AdvancedTab settings={settings} set={set} saving={false} />);

    const recordButtons = screen.getAllByRole("button", { name: "ShortcutRecordButton" });
    expect(recordButtons).toHaveLength(2);
    fireEvent.click(recordButtons[0]);
    fireEvent.keyDown(window, {
      key: "K",
      code: "KeyK",
      ctrlKey: true,
      shiftKey: true,
    });

    await waitFor(() =>
      expect(tauriMocks.validateGlobalShortcut).toHaveBeenCalledWith("Ctrl+Shift+K"),
    );
    expect(set).toHaveBeenCalledWith({ globalShortcut: "Ctrl+Shift+K" });
  });

  it("does not persist an unavailable taskbar shortcut", async () => {
    const set = vi.fn();
    tauriMocks.validateGlobalShortcut.mockRejectedValue(
      new Error("Shortcut is already in use"),
    );

    render(<AdvancedTab settings={settings} set={set} saving={false} />);
    const recordButtons = screen.getAllByRole("button", { name: "ShortcutRecordButton" });
    fireEvent.click(recordButtons[1]);
    fireEvent.keyDown(window, {
      key: "K",
      code: "KeyK",
      ctrlKey: true,
      shiftKey: true,
    });

    await waitFor(() =>
      expect(screen.getByText("Shortcut is already in use")).toBeInTheDocument(),
    );
    expect(set).not.toHaveBeenCalledWith({ taskbarToggleShortcut: "Ctrl+Shift+K" });
  });

  it("leaves an unchanged shortcut alone", async () => {
    const set = vi.fn();
    render(<AdvancedTab settings={settings} set={set} saving={false} />);

    const recordButtons = screen.getAllByRole("button", { name: "ShortcutRecordButton" });
    fireEvent.click(recordButtons[1]);
    fireEvent.keyDown(window, {
      key: "H",
      code: "KeyH",
      ctrlKey: true,
      shiftKey: true,
    });

    await waitFor(() => expect(tauriMocks.validateGlobalShortcut).not.toHaveBeenCalled());
    expect(set).not.toHaveBeenCalled();
  });

  it("clears only the selected shortcut", () => {
    const set = vi.fn();
    render(<AdvancedTab settings={settings} set={set} saving={false} />);

    const clearButtons = screen.getAllByRole("button", { name: "ShortcutClearButton" });
    expect(clearButtons).toHaveLength(2);
    fireEvent.click(clearButtons[1]);

    expect(set).toHaveBeenCalledWith({ taskbarToggleShortcut: "" });
    expect(set).not.toHaveBeenCalledWith({ globalShortcut: "" });
  });
});
