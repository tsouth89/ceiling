import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import ChartsPanel from "./ChartsPanel";
import type { ProviderUsageSnapshot } from "../types/bridge";

// Stub the async, backend-fetching ChartsSection so this test exercises only
// ChartsPanel's own selection logic.
vi.mock("./settings/providers/sections/charts/ChartsSection", () => ({
  ChartsSection: ({ providerId }: { providerId: string }) => (
    <div data-testid="charts-section">{providerId}</div>
  ),
}));
vi.mock("./ProviderComparison", () => ({
  default: () => <div data-testid="provider-comparison">compare</div>,
}));
vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({ t: (k: string) => k }),
}));

function provider(
  overrides: Partial<ProviderUsageSnapshot> = {},
): ProviderUsageSnapshot {
  const win = {
    usedPercent: 20,
    remainingPercent: 80,
    windowMinutes: null,
    resetsAt: null,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
    reserveWillLastToReset: false,
    reserveEtaSeconds: null,
  };
  return {
    providerId: "codex",
    displayName: "Codex",
    primary: win,
    primaryLabel: "Weekly",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    inactiveRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "web",
    updatedAt: new Date().toISOString(),
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
    ...overrides,
  };
}

describe("ChartsPanel", () => {
  it("shows an empty state when no provider reports chart data", () => {
    const { container, getByText } = render(
      <ChartsPanel
        providers={[provider({ providerId: "copilot", displayName: "Copilot" })]}
      />,
    );
    expect(getByText("No charts yet")).toBeTruthy();
    expect(container.querySelector('[data-testid="charts-section"]')).toBeNull();
  });

  it("defaults to comparison when Codex and Claude are both available", () => {
    const { getAllByRole, getByTestId, queryByTestId } = render(
      <ChartsPanel
        providers={[
          provider({ providerId: "codex", displayName: "Codex" }),
          provider({ providerId: "claude", displayName: "Claude" }),
          provider({ providerId: "cursor", displayName: "Cursor" }),
        ]}
      />,
    );
    const tabs = getAllByRole("tab");
    expect(tabs).toHaveLength(4);
    expect(getByTestId("provider-comparison").textContent).toBe("compare");
    expect(queryByTestId("charts-section")).toBeNull();
  });

  it("switches the charts when another provider is selected", () => {
    const { getByRole, getByTestId } = render(
      <ChartsPanel
        providers={[
          provider({ providerId: "codex", displayName: "Codex" }),
          provider({ providerId: "claude", displayName: "Claude" }),
        ]}
      />,
    );
    expect(getByRole("tab", { name: /Compare/ }).getAttribute("aria-selected")).toBe("true");
    fireEvent.click(getByRole("tab", { name: /Claude/ }));
    expect(getByTestId("charts-section").textContent).toBe("claude");
  });

  it("omits the selector when only one provider is supported", () => {
    const { queryAllByRole, getByTestId } = render(
      <ChartsPanel
        providers={[
          provider({ providerId: "claude", displayName: "Claude" }),
          provider({ providerId: "copilot", displayName: "Copilot" }),
        ]}
      />,
    );
    expect(queryAllByRole("tab")).toHaveLength(0);
    expect(getByTestId("charts-section").textContent).toBe("claude");
  });
});
