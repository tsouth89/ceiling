import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const eventMocks = vi.hoisted(() => {
  const listeners: Record<string, () => void> = {};
  return {
    listeners,
    listen: vi.fn((name: string, cb: () => void) => {
      listeners[name] = cb;
      return Promise.resolve(() => {
        delete listeners[name];
      });
    }),
  };
});

const tauriMocks = vi.hoisted(() => ({
  getSettingsSnapshot: vi.fn(),
  updateSettings: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => eventMocks);
vi.mock("../lib/tauri", () => tauriMocks);

import { useSettings } from "./useSettings";
import type { SettingsSnapshot } from "../types/bridge";

const snapshot = (windowScalePercent: number) =>
  ({ windowScalePercent }) as unknown as SettingsSnapshot;

describe("useSettings live sync", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("re-fetches the snapshot when settings-changed fires from another window", async () => {
    tauriMocks.getSettingsSnapshot.mockResolvedValue(snapshot(100));
    // Stable identity: the hook's bootstrap effect keys on `initial`, so a new
    // object each render would loop forever.
    const initial = snapshot(100);
    const { result } = renderHook(() => useSettings(initial));

    // The hook registers a "settings-changed" listener.
    await waitFor(() =>
      expect(eventMocks.listeners["settings-changed"]).toBeTypeOf("function"),
    );

    // A change persisted by the detached Settings window bumps the scale.
    tauriMocks.getSettingsSnapshot.mockResolvedValue(snapshot(175));
    await act(async () => {
      eventMocks.listeners["settings-changed"]();
    });

    await waitFor(() =>
      expect(result.current.settings.windowScalePercent).toBe(175),
    );
  });

  it("unsubscribes the listener on unmount", async () => {
    const unlisten = vi.fn();
    eventMocks.listen.mockResolvedValueOnce(unlisten);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(snapshot(100));

    const initial = snapshot(100);
    const { unmount } = renderHook(() => useSettings(initial));
    await waitFor(() => expect(eventMocks.listen).toHaveBeenCalled());

    unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalledTimes(1));
  });

  it("serializes rapid updates so an older response cannot overwrite a newer choice", async () => {
    let resolveFirst!: (value: SettingsSnapshot) => void;
    let resolveSecond!: (value: SettingsSnapshot) => void;
    tauriMocks.getSettingsSnapshot.mockResolvedValue(snapshot(100));
    tauriMocks.updateSettings
      .mockImplementationOnce(
        () =>
          new Promise<SettingsSnapshot>((resolve) => {
            resolveFirst = resolve;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise<SettingsSnapshot>((resolve) => {
            resolveSecond = resolve;
          }),
      );

    const initial = snapshot(100);
    const { result } = renderHook(() => useSettings(initial));

    let first!: Promise<void>;
    let second!: Promise<void>;
    act(() => {
      first = result.current.update({ windowScalePercent: 125 });
      second = result.current.update({ windowScalePercent: 150 });
    });

    await waitFor(() => expect(tauriMocks.updateSettings).toHaveBeenCalledTimes(1));
    expect(result.current.saving).toBe(true);

    resolveFirst(snapshot(125));
    await first;
    await waitFor(() => expect(tauriMocks.updateSettings).toHaveBeenCalledTimes(2));
    expect(result.current.saving).toBe(true);

    resolveSecond(snapshot(150));
    await second;
    await waitFor(() => {
      expect(result.current.settings.windowScalePercent).toBe(150);
      expect(result.current.saving).toBe(false);
    });
  });
});
