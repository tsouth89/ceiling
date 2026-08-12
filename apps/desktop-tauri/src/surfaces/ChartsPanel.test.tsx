import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import ChartsPanel, { chartSectionKey } from "./ChartsPanel";
import type { ProviderUsageSnapshot } from "../types/bridge";

// Stub the async, backend-fetching ChartsSection so this test exercises only
// ChartsPanel's own selection logic.
vi.mock("./settings/providers/sections/charts/ChartsSection", () => ({
  ChartsSection: ({
    providerId,
    accountEmail,
    accountId,
  }: {
    providerId: string;
    accountEmail?: string | null;
    accountId?: string | null;
  }) => (
    <div data-testid="charts-section">
      {providerId}
      {accountEmail ? `:${accountEmail}` : ""}
      {accountId ? `:${accountId}` : ""}
    </div>
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
  it("remounts charts when fallback account identity changes", () => {
    const personal = provider({ accountEmail: "personal@example.com" });
    const work = provider({ accountEmail: "work@example.com" });
    const organization = provider({
      accountEmail: null,
      accountOrganization: "org-work",
    });

    expect(chartSectionKey(personal)).not.toBe(chartSectionKey(work));
    expect(chartSectionKey(organization)).toBe("codex:org-work");
  });

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

  it("is one tab stop and arrows across the strip without loading each provider", () => {
    const { getByRole, getByTestId } = render(
      <ChartsPanel
        providers={[
          provider({ providerId: "codex", displayName: "Codex" }),
          provider({ providerId: "claude", displayName: "Claude" }),
        ]}
      />,
    );
    const compare = getByRole("tab", { name: /Compare/ });
    const claude = getByRole("tab", { name: /Claude/ });
    expect(compare.tabIndex).toBe(0);
    expect(claude.tabIndex).toBe(-1);

    // Manual activation: arrowing past Codex must not mount its charts.
    fireEvent.keyDown(compare, { key: "End" });
    expect(claude).toBe(document.activeElement);
    expect(getByTestId("provider-comparison")).toBeTruthy();

    fireEvent.click(claude);
    expect(getByTestId("charts-section").textContent).toBe("claude");
    expect(claude.tabIndex).toBe(0);
    expect(compare.tabIndex).toBe(-1);
  });

  it("names the charts body as the panel of the selected provider tab", () => {
    const { getByRole } = render(
      <ChartsPanel
        providers={[
          provider({ providerId: "codex", displayName: "Codex" }),
          provider({ providerId: "claude", displayName: "Claude" }),
        ]}
      />,
    );
    const panel = getByRole("tabpanel");
    const compare = getByRole("tab", { name: /Compare/ });
    expect(compare.getAttribute("aria-controls")).toBe(panel.id);
    expect(panel.getAttribute("aria-labelledby")).toBe(compare.id);
  });

  it("shows and selects each configured account independently", () => {
    const { getAllByRole, getByTestId } = render(
      <ChartsPanel
        providers={[
          provider({
            providerId: "codex",
            displayName: "Codex",
            accountId: "acct-personal",
            accountEmail: "tsouth2@gmail.com",
          }),
          provider({
            providerId: "codex",
            displayName: "Codex",
            accountId: "acct-work",
            accountEmail: "bts@cssi.us",
          }),
          provider({ providerId: "claude", displayName: "Claude" }),
        ]}
      />,
    );

    expect(getAllByRole("tab", { name: /Codex/ })).toHaveLength(2);
    expect(getAllByRole("tab", { name: /Claude/ })).toHaveLength(1);

    fireEvent.click(getAllByRole("tab", { name: /Codex — bts@cssi\.us/ })[0]);
    expect(getByTestId("charts-section").textContent).toBe(
      "codex:bts@cssi.us:acct-work",
    );
  });

  it("masks account identities in tabs when personal info is hidden", () => {
    const { getAllByRole } = render(
      <ChartsPanel
        hideEmail
        providers={[
          provider({
            accountId: "acct-personal",
            accountEmail: "personal@example.com",
          }),
          provider({
            accountId: "acct-work",
            accountEmail: "work@example.com",
          }),
        ]}
      />,
    );

    const names = getAllByRole("tab").map((tab) => tab.textContent ?? "");
    expect(names.join(" ")).not.toContain("personal@example.com");
    expect(names.join(" ")).not.toContain("work@example.com");
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
