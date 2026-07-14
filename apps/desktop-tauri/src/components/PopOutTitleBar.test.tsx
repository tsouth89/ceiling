import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const windowMocks = vi.hoisted(() => {
  const toggleMaximize = vi.fn().mockResolvedValue(undefined);
  const close = vi.fn().mockResolvedValue(undefined);
  const isMaximized = vi.fn().mockResolvedValue(false);
  const onResized = vi.fn().mockResolvedValue(() => {});
  return {
    toggleMaximize,
    close,
    isMaximized,
    onResized,
    getCurrentWindow: vi.fn(() => ({
      toggleMaximize,
      close,
      isMaximized,
      onResized,
    })),
  };
});

const bridgeMocks = vi.hoisted(() => ({
  hideDashboardToTray: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/window", () => windowMocks);
vi.mock("../lib/tauri", () => bridgeMocks);
// Bypass the LocaleProvider: t(key) returns the key, so aria-labels are the
// key names (e.g. "WindowMinimize").
vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key, language: "english" }),
}));

import PopOutTitleBar from "./PopOutTitleBar";

describe("PopOutTitleBar", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("hides minimize to the tray and keeps maximize and close native", async () => {
    render(<PopOutTitleBar />);

    fireEvent.click(await screen.findByRole("button", { name: "WindowMinimize" }));
    fireEvent.click(screen.getByRole("button", { name: "WindowMaximize" }));
    fireEvent.click(screen.getByRole("button", { name: "WindowClose" }));

    expect(bridgeMocks.hideDashboardToTray).toHaveBeenCalledTimes(1);
    expect(windowMocks.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(windowMocks.close).toHaveBeenCalledTimes(1);
  });

  it("announces Restore on the middle control once the window is maximized", async () => {
    windowMocks.isMaximized.mockResolvedValueOnce(true);
    render(<PopOutTitleBar />);

    expect(
      await screen.findByRole("button", { name: "WindowRestore" }),
    ).toBeTruthy();
  });

  it("toggles maximize when the title bar is double-clicked", () => {
    const { container } = render(<PopOutTitleBar />);
    const titleBar = container.querySelector(".popout-titlebar") as HTMLElement;

    fireEvent.doubleClick(titleBar);

    expect(windowMocks.toggleMaximize).toHaveBeenCalledTimes(1);
  });
});
