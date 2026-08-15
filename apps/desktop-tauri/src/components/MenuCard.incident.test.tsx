import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderIncident, ProviderUsageSnapshot } from "../types/bridge";

const openProviderStatusPage = vi.fn(() => Promise.resolve());
vi.mock("../lib/tauri", () => ({
  getProviderChartData: vi.fn(() => Promise.resolve(null)),
  openProviderStatusPage: (id: string) => openProviderStatusPage(id),
}));
vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

const MenuCard = (await import("./MenuCard")).default;

const provider = (over: Partial<ProviderUsageSnapshot> = {}): ProviderUsageSnapshot =>
  ({
    providerId: "codex",
    displayName: "Codex",
    updatedAt: new Date().toISOString(),
    primary: { usedPercent: 40, label: "Session" },
    inactiveRateWindows: [],
    extraRateWindows: [],
    promoSignals: [],
    ...over,
  }) as unknown as ProviderUsageSnapshot;

const incident: ProviderIncident = {
  providerId: "codex",
  severity: "major",
  description: "Major Outage",
  statusPageUrl: "https://status.openai.com",
};

const renderCard = (over: Partial<ProviderUsageSnapshot> = {}) =>
  render(
    <MenuCard
      provider={provider(over)}
      hideEmail={false}
      resetTimeRelative
      incident={incident}
    />,
  );

describe("MenuCard incident banner", () => {
  it("shows the provider's own wording and opens its status page", () => {
    openProviderStatusPage.mockClear();
    renderCard();

    expect(screen.getByText("Major Outage")).toBeInTheDocument();

    // The URL used to be plain text, so the badge told you an outage was on
    // and gave you no way to go and check it.
    fireEvent.click(screen.getByRole("button", { name: "IncidentStatusPage" }));
    expect(openProviderStatusPage).toHaveBeenCalledWith("codex");
  });

  /// A provider mid-outage often fails its fetch too, and the error block
  /// replaces the whole meta row. That is when the distinction matters most.
  it("still shows the banner when the provider read failed", () => {
    renderCard({ error: "Request timed out" });

    expect(screen.getByText("Major Outage")).toBeInTheDocument();
    expect(screen.getByText("Request timed out")).toBeInTheDocument();
  });

  it("shows nothing when the provider is operational", () => {
    render(
      <MenuCard provider={provider()} hideEmail={false} resetTimeRelative incident={null} />,
    );

    expect(screen.queryByText("Major Outage")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "IncidentStatusPage" }),
    ).not.toBeInTheDocument();
  });
});
