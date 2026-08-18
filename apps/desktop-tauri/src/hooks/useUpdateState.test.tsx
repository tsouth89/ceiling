import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

const tauriMocks = vi.hoisted(() => ({
  checkForUpdates: vi.fn(),
  downloadUpdate: vi.fn(),
  applyUpdate: vi.fn(),
  dismissUpdate: vi.fn(),
  openReleasePage: vi.fn(),
  getUpdateState: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => eventMocks);
vi.mock("../lib/tauri", () => tauriMocks);

import { useUpdateState } from "./useUpdateState";

const idle = {
  status: "idle" as const,
  version: null,
  error: null,
  progress: null,
  releaseUrl: null,
  canDownload: false,
  canApply: false,
  lastCheckedAt: null,
};

describe("useUpdateState", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getUpdateState.mockResolvedValue(idle);
  });

  it("surfaces an invoke failure as Error, not Idle", async () => {
    tauriMocks.checkForUpdates.mockRejectedValue(new Error("bridge down"));

    const { result } = renderHook(() => useUpdateState());
    await waitFor(() => expect(tauriMocks.getUpdateState).toHaveBeenCalled());

    act(() => {
      result.current.checkNow();
    });

    await waitFor(() => {
      expect(result.current.updateState.status).toBe("error");
    });
    expect(result.current.updateState.error).toBe("bridge down");
    expect(result.current.updateState.status).not.toBe("idle");
  });

  it("keeps a downloaded update on screen when a later check cannot run", async () => {
    // The backend still holds the installer. Turning the surface into an error
    // took Install and Restart away from an update sitting ready on disk.
    tauriMocks.getUpdateState.mockResolvedValue({
      ...idle,
      status: "ready",
      version: "9.9.9",
      canApply: true,
    });
    tauriMocks.checkForUpdates.mockRejectedValue(new Error("bridge down"));

    const { result } = renderHook(() => useUpdateState());
    await waitFor(() => expect(result.current.updateState.status).toBe("ready"));

    act(() => {
      result.current.checkNow();
    });

    await waitFor(() => expect(tauriMocks.checkForUpdates).toHaveBeenCalled());
    expect(result.current.updateState.status).toBe("ready");
    expect(result.current.updateState.canApply).toBe(true);
  });
});
