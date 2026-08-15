import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ActivityHeatmap, ActivityHourPoint } from "../types/bridge";

const getLocalActivityHeatmap = vi.fn();
vi.mock("../lib/tauri", () => ({
  getLocalActivityHeatmap: () => getLocalActivityHeatmap(),
}));

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

  it("surfaces a failed read instead of drawing an empty grid", async () => {
    getLocalActivityHeatmap.mockRejectedValue(new Error("scan died"));
    render(<ActivityHeatmapCard />);

    await waitFor(() => expect(screen.getByText("scan died")).toBeInTheDocument());
    expect(document.querySelectorAll(".activity-card__cell--day")).toHaveLength(0);
  });
});
