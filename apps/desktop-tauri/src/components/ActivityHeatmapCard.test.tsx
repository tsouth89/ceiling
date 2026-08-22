import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ActivityHeatmap, ActivityHourPoint } from "../types/bridge";

const getLocalActivityHeatmap = vi.fn();
vi.mock("../lib/tauri", () => ({
  getLocalActivityHeatmap: () => getLocalActivityHeatmap(),
}));

const eventMocks = vi.hoisted(() => {
  const handlers: Record<string, (event: { payload: unknown }) => void> = {};
  return {
    handlers,
    listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
      handlers[name] = handler;
      return Promise.resolve(() => {
        delete handlers[name];
      });
    }),
    emit(name: string, payload: unknown) {
      handlers[name]?.({ payload });
    },
  };
});

vi.mock("@tauri-apps/api/event", () => ({ listen: eventMocks.listen }));


const { ActivityHeatmapCard } = await import("./ActivityHeatmapCard");

const hour = (over: Partial<ActivityHourPoint> = {}): ActivityHourPoint => ({
  providerId: "codex",
  date: "2026-08-15",
  hour: 9,
  apiValueUsd: 4,
  tokens: 1000,
  calls: 2,
  ...over,
});

const heatmap = (over: Partial<ActivityHeatmap> = {}): ActivityHeatmap => ({
  days: ["2026-08-14", "2026-08-15"],
  providerIds: ["codex", "claude"],
  hours: [hour(), hour({ providerId: "claude", date: "2026-08-14", hour: 22 })],
  timezoneLabel: "UTC-07:00",
  ...over,
});

const activeDayCells = () =>
  [...document.querySelectorAll(".activity-card__cell--day")].filter(
    (cell) => cell.getAttribute("data-level") !== "0",
  ).length;

describe("ActivityHeatmapCard", () => {
  beforeEach(() => {
    getLocalActivityHeatmap.mockReset();
  });

  it("draws a cell per day in range and lights the active ones", async () => {
    getLocalActivityHeatmap.mockResolvedValue(heatmap());
    render(<ActivityHeatmapCard />);

    await waitFor(() => expect(screen.getByText("When you work")).toBeInTheDocument());
    expect(document.querySelectorAll(".activity-card__cell--day")).toHaveLength(2);
    expect(activeDayCells()).toBe(2);
    // The clock the buckets use is disclosed, not assumed.
    expect(screen.getByText(/UTC-07:00/)).toBeInTheDocument();
  });

  it("drops a provider's hours when its chip is switched off", async () => {
    getLocalActivityHeatmap.mockResolvedValue(heatmap());
    render(<ActivityHeatmapCard />);
    await waitFor(() => expect(activeDayCells()).toBe(2));

    fireEvent.click(screen.getByRole("button", { name: /Claude/ }));

    await waitFor(() => expect(activeDayCells()).toBe(1));
  });

  /// Regression: an empty visible set used to mean "no filter", so turning
  /// every chip off showed every provider instead of an empty grid.
  it("empties the grid when every provider is switched off", async () => {
    getLocalActivityHeatmap.mockResolvedValue(heatmap());
    render(<ActivityHeatmapCard />);
    await waitFor(() => expect(activeDayCells()).toBe(2));

    fireEvent.click(screen.getByRole("button", { name: /Claude/ }));
    fireEvent.click(screen.getByRole("button", { name: /Codex/ }));

    await waitFor(() => expect(activeDayCells()).toBe(0));
    // And it says why the grid is empty, rather than blaming the machine.
    expect(screen.getByText(/Every provider is hidden/)).toBeInTheDocument();
  });

  it("says the window is empty when there is genuinely no activity", async () => {
    getLocalActivityHeatmap.mockResolvedValue(
      heatmap({ providerIds: [], hours: [] }),
    );
    render(<ActivityHeatmapCard />);

    await waitFor(() =>
      expect(screen.getByText(/No local activity in this window yet/)).toBeInTheDocument(),
    );
  });

  it("picks up a background rescan without being remounted", async () => {
    getLocalActivityHeatmap.mockResolvedValueOnce(heatmap());
    render(<ActivityHeatmapCard />);
    await waitFor(() => expect(getLocalActivityHeatmap).toHaveBeenCalledTimes(1));

    getLocalActivityHeatmap.mockResolvedValueOnce(heatmap());
    eventMocks.emit("local-scan-refreshed", "activity-heatmap");

    await waitFor(() => expect(getLocalActivityHeatmap).toHaveBeenCalledTimes(2));
  });

  it("surfaces a failed read instead of drawing an empty grid", async () => {
    getLocalActivityHeatmap.mockRejectedValue(new Error("scan died"));
    render(<ActivityHeatmapCard />);

    await waitFor(() => expect(screen.getByText("scan died")).toBeInTheDocument());
    expect(document.querySelectorAll(".activity-card__cell--day")).toHaveLength(0);
  });

  /// SBS-945: 29 quiet days and one spike used to share data-level 2, so the
  /// calendar painted the busiest day the same shade as the quietest.
  it("paints the spike day at the legend's top swatch", async () => {
    const days = Array.from({ length: 30 }, (_, index) => {
      const day = String(index + 1).padStart(2, "0");
      return `2026-08-${day}`;
    });
    const hours = days.map((date, index) =>
      hour({
        date,
        apiValueUsd: index === 29 ? 500 : 1,
        tokens: 1,
        calls: 1,
        providerId: "codex",
      }),
    );
    getLocalActivityHeatmap.mockResolvedValue(
      heatmap({ days, hours, providerIds: ["codex"] }),
    );
    render(<ActivityHeatmapCard />);

    await waitFor(() =>
      expect(document.querySelectorAll(".activity-card__cell--day")).toHaveLength(30),
    );
    const levels = [...document.querySelectorAll(".activity-card__cell--day")].map((cell) =>
      cell.getAttribute("data-level"),
    );
    expect(levels[0]).toBe("2");
    expect(levels[28]).toBe("2");
    expect(levels[29]).toBe("4");
  });
});
