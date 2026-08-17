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
});
