import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderIncident } from "../types/bridge";

const getProviderIncidents = vi.fn();
vi.mock("../lib/tauri", () => ({
  getProviderIncidents: () => getProviderIncidents(),
}));

const { useProviderIncidents, resetProviderIncidentsCache } = await import(
  "./useProviderIncidents"
);

const incident = (over: Partial<ProviderIncident> = {}): ProviderIncident => ({
  providerId: "codex",
  severity: "major",
  description: "Major Outage",
  statusPageUrl: "https://status.openai.com",
  ...over,
});

describe("useProviderIncidents", () => {
  beforeEach(() => {
    resetProviderIncidentsCache();
    getProviderIncidents.mockReset();
  });

  afterEach(() => {
    resetProviderIncidentsCache();
  });

  it("asks for nothing while the feature is off", () => {
    const { result } = renderHook(() => useProviderIncidents(false));

    expect(getProviderIncidents).not.toHaveBeenCalled();
    expect(result.current).toEqual({});
  });

  it("returns the incidents the backend reports", async () => {
    getProviderIncidents.mockResolvedValue({ codex: incident() });

    const { result } = renderHook(() => useProviderIncidents(true));

    await waitFor(() => expect(result.current.codex?.severity).toBe("major"));
  });

  /// A grid of eight provider cards must not make eight identical calls.
  it("shares one fetch across every consumer", async () => {
    getProviderIncidents.mockResolvedValue({ codex: incident() });

    const first = renderHook(() => useProviderIncidents(true));
    const second = renderHook(() => useProviderIncidents(true));

    await waitFor(() => expect(first.result.current.codex).toBeDefined());
    await waitFor(() => expect(second.result.current.codex).toBeDefined());
    expect(getProviderIncidents).toHaveBeenCalledTimes(1);
  });

  it("keeps the last good answer when a poll fails", async () => {
    getProviderIncidents.mockResolvedValueOnce({ codex: incident() });
    const { result } = renderHook(() => useProviderIncidents(true));
    await waitFor(() => expect(result.current.codex).toBeDefined());

    // A failure must not blank a badge for an incident that is still live.
    getProviderIncidents.mockRejectedValueOnce(new Error("offline"));
    const second = renderHook(() => useProviderIncidents(true));
    await waitFor(() => expect(second.result.current.codex).toBeDefined());
  });

  /// A first-open timeout used to stamp loadedAt, so the empty answer was
  /// served for the full refresh window -- three times the backend's own
  /// error backoff.
  it("retries after a failed first poll instead of caching the miss", async () => {
    getProviderIncidents.mockRejectedValueOnce(new Error("timeout"));
    const first = renderHook(() => useProviderIncidents(true));
    await waitFor(() => expect(getProviderIncidents).toHaveBeenCalledTimes(1));
    expect(first.result.current).toEqual({});

    getProviderIncidents.mockResolvedValueOnce({ codex: incident() });
    const second = renderHook(() => useProviderIncidents(true));
    await waitFor(() => expect(second.result.current.codex).toBeDefined());
    expect(getProviderIncidents).toHaveBeenCalledTimes(2);
  });

  /// An empty map usually means "nothing is wrong", but it is also what comes
  /// back on a cold start where every status page failed its first read and
  /// there was no earlier answer to carry forward. Holding that for the full
  /// quarter hour sits on the miss long past the backend's own five-minute
  /// backoff.
  it("re-asks after five minutes when the answer was empty", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-08-15T12:00:00Z"));
    getProviderIncidents.mockResolvedValueOnce({});

    const first = renderHook(() => useProviderIncidents(true));
    await waitFor(() => expect(getProviderIncidents).toHaveBeenCalledTimes(1));
    expect(first.result.current).toEqual({});

    vi.setSystemTime(new Date("2026-08-15T12:06:00Z"));
    getProviderIncidents.mockResolvedValueOnce({ codex: incident() });
    const second = renderHook(() => useProviderIncidents(true));

    await waitFor(() => expect(second.result.current.codex).toBeDefined());
    expect(getProviderIncidents).toHaveBeenCalledTimes(2);
    vi.useRealTimers();
  });

  /// The short window is only for an empty answer. A live badge is a real
  /// reading and keeps the full TTL, so the badge does not re-poll every five
  /// minutes for the whole length of an outage.
  it("holds a known incident for the full refresh window", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-08-15T12:00:00Z"));
    getProviderIncidents.mockResolvedValueOnce({ codex: incident() });

    const first = renderHook(() => useProviderIncidents(true));
    await waitFor(() => expect(first.result.current.codex).toBeDefined());

    vi.setSystemTime(new Date("2026-08-15T12:06:00Z"));
    renderHook(() => useProviderIncidents(true));

    expect(getProviderIncidents).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });

  it("reports nothing once the feature is switched back off", async () => {
    getProviderIncidents.mockResolvedValue({ codex: incident() });
    const { result, rerender } = renderHook(
      ({ on }: { on: boolean }) => useProviderIncidents(on),
      { initialProps: { on: true } },
    );
    await waitFor(() => expect(result.current.codex).toBeDefined());

    rerender({ on: false });
    expect(result.current).toEqual({});
  });
});
