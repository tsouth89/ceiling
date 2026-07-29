import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getLocalApiValueTotals: vi.fn(),
}));

vi.mock("../lib/tauri", () => tauriMocks);

import { TotalApiValueCard } from "./TotalApiValueCard";

function period(over: Record<string, number | boolean>) {
  return {
    apiValueUsd: 0,
    tokens: 0,
    pricedTokens: 0,
    totalTokens: 0,
    hasData: false,
    ...over,
  };
}

const twoProviders = [
  {
    providerId: "codex",
    today: period({ apiValueUsd: 90, tokens: 9000, pricedTokens: 8000, totalTokens: 9000, hasData: true }),
    yesterday: period({}),
    thirtyDays: period({ apiValueUsd: 300, tokens: 30000, pricedTokens: 30000, totalTokens: 30000, hasData: true }),
    priorThirtyDays: period({}),
  },
  {
    providerId: "claude",
    today: period({ apiValueUsd: 30, tokens: 3000, pricedTokens: 3000, totalTokens: 3000, hasData: true }),
    yesterday: period({}),
    thirtyDays: period({ apiValueUsd: 100, tokens: 10000, pricedTokens: 10000, totalTokens: 10000, hasData: true }),
    priorThirtyDays: period({}),
  },
];

describe("TotalApiValueCard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows the aggregate total, ranked legend, and pricing coverage", async () => {
    tauriMocks.getLocalApiValueTotals.mockResolvedValue(twoProviders);
    const { getByLabelText, getByText } = render(<TotalApiValueCard />);

    await waitFor(() => expect(getByText("$120.00")).toBeTruthy()); // 90 + 30, Today
    const ring = getByLabelText(/API value for Today/i);
    expect(ring).toBeTruthy();
    // Ranked legend: Codex leads at 75%, Claude at 25%.
    expect(getByText("Codex")).toBeTruthy();
    expect(getByText("Claude")).toBeTruthy();
    expect(getByText("75%")).toBeTruthy();
    expect(getByText("25%")).toBeTruthy();
    // Codex has unpriced tokens today (8000 of 9000) -> coverage 12000/12000? No:
    // priced 8000+3000=11000 of 12000 total = 92%.
    expect(getByText(/92% of tokens priced/)).toBeTruthy();
    expect(getByText(/unpriced models in Codex/)).toBeTruthy();
  });

  it('shows "No data" for a period with no provider data', async () => {
    tauriMocks.getLocalApiValueTotals.mockResolvedValue([
      {
        providerId: "codex",
        today: period({ apiValueUsd: 5, tokens: 500, pricedTokens: 500, totalTokens: 500, hasData: true }),
        yesterday: period({}),
        thirtyDays: period({ apiValueUsd: 5, tokens: 500, pricedTokens: 500, totalTokens: 500, hasData: true }),
        priorThirtyDays: period({}),
      },
    ]);
    const { getByText, getAllByText, findByText } = render(<TotalApiValueCard />);
    // Default period is Today (has data); $5.00 appears in the ring and legend.
    await waitFor(() => expect(getAllByText("$5.00").length).toBeGreaterThan(0));
    getByText("Yesterday").click();
    expect(await findByText(/No data for Yesterday/)).toBeTruthy();
  });

  it("surfaces an unavailable state when the command fails", async () => {
    tauriMocks.getLocalApiValueTotals.mockRejectedValue(new Error("boom"));
    const { findByText } = render(<TotalApiValueCard />);
    expect(await findByText(/unavailable right now/)).toBeTruthy();
  });

  it("loads a custom date range when Custom is selected", async () => {
    tauriMocks.getLocalApiValueTotals
      .mockResolvedValueOnce([
        {
          providerId: "codex",
          today: period({
            apiValueUsd: 1,
            tokens: 10,
            pricedTokens: 10,
            totalTokens: 10,
            hasData: true,
          }),
          yesterday: period({}),
          thirtyDays: period({}),
          priorThirtyDays: period({}),
        },
      ])
      .mockResolvedValue([
        {
          providerId: "codex",
          today: period({
            apiValueUsd: 1,
            tokens: 10,
            pricedTokens: 10,
            totalTokens: 10,
            hasData: true,
          }),
          yesterday: period({}),
          thirtyDays: period({}),
          priorThirtyDays: period({}),
          custom: period({
            apiValueUsd: 55,
            tokens: 5000,
            pricedTokens: 5000,
            totalTokens: 5000,
            hasData: true,
          }),
          dailySeries: [
            { date: "2026-07-01", apiValueUsd: 20, tokens: 100 },
            { date: "2026-07-15", apiValueUsd: 35, tokens: 200 },
          ],
        },
      ]);
    const { getByText, getAllByText, getByLabelText, findAllByText } = render(
      <TotalApiValueCard />,
    );
    await waitFor(() => expect(getAllByText("$1.00").length).toBeGreaterThan(0));
    getByText("Custom").click();
    await waitFor(() =>
      expect(tauriMocks.getLocalApiValueTotals).toHaveBeenCalledWith(
        expect.objectContaining({
          since: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/),
          until: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/),
        }),
      ),
    );
    expect(getByLabelText("Custom date range")).toBeTruthy();
    expect((await findAllByText("$55.00")).length).toBeGreaterThan(0);
  });

});
