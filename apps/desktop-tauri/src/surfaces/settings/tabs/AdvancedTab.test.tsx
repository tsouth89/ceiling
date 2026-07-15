import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  registerGlobalShortcut: vi.fn(),
  unregisterGlobalShortcut: vi.fn(),
}));

vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

vi.mock("../../../lib/tauri", () => tauriMocks);

import AdvancedTab from "./AdvancedTab";
import type { SettingsSnapshot } from "../../../types/bridge";

const settings = {
  globalShortcut: "Ctrl+Shift+U",
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

  it("shows registration failures and does not persist a shortcut that is inactive", async () => {
    const set = vi.fn();
    tauriMocks.registerGlobalShortcut.mockRejectedValue(
      new Error("Shortcut is already in use"),
    );

    render(<AdvancedTab settings={settings} set={set} saving={false} />);
    fireEvent.click(screen.getByRole("button", { name: "ShortcutRecordButton" }));
    fireEvent.keyDown(window, {
      key: "K",
      code: "KeyK",
      ctrlKey: true,
      shiftKey: true,
    });

    await waitFor(() =>
      expect(screen.getByText("Shortcut is already in use")).toBeInTheDocument(),
    );
    expect(set).not.toHaveBeenCalledWith({ globalShortcut: "Ctrl+Shift+K" });
  });

  it("keeps the saved shortcut when unregistering fails", async () => {
    const set = vi.fn();
    tauriMocks.unregisterGlobalShortcut.mockRejectedValue(
      new Error("Could not unregister shortcut"),
    );

    render(<AdvancedTab settings={settings} set={set} saving={false} />);
    fireEvent.click(screen.getByRole("button", { name: "ShortcutClearButton" }));

    await waitFor(() =>
      expect(screen.getByText("Could not unregister shortcut")).toBeInTheDocument(),
    );
    expect(set).not.toHaveBeenCalledWith({ globalShortcut: "" });
  });
});
