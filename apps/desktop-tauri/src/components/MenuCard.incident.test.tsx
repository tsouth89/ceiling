import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderIncident, ProviderUsageSnapshot } from "../types/bridge";

const openExternalUrl = vi.fn((_url: string) => Promise.resolve());
vi.mock("../lib/tauri", () => ({
  getProviderChartData: vi.fn(() => Promise.resolve(null)),
  openExternalUrl: (url: string) => openExternalUrl(url),
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
    openExternalUrl.mockClear();
    renderCard();

    expect(screen.getByText("Major Outage")).toBeInTheDocument();

    // The URL used to be plain text, so the badge told you an outage was on
    // and gave you no way to go and check it. It then routed through the
    // provider registry, where Cursor has no status_page_url and the button
    // was silently dead. It opens the incident's own URL now.
    fireEvent.click(screen.getByRole("button", { name: "IncidentStatusPage" }));
    expect(openExternalUrl).toHaveBeenCalledWith("https://status.openai.com");
  });

  /// Cursor is polled through `get_status_page_url`, but its ProviderMetadata
  /// sets `status_page_url: None`. Routing the button through the registry by
  /// provider id therefore returned an error the void handler swallowed, so
  /// the control did nothing at all for Cursor.
  it("opens the incident's own URL for a provider the registry has none for", () => {
    openExternalUrl.mockClear();
    render(
      <MenuCard
        provider={provider({ providerId: "cursor", displayName: "Cursor" })}
        hideEmail={false}
        resetTimeRelative
        incident={{
          providerId: "cursor",
          severity: "degraded",
          description: "Degraded Performance",
          statusPageUrl: "https://status.cursor.com",
        }}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "IncidentStatusPage" }));
    expect(openExternalUrl).toHaveBeenCalledWith("https://status.cursor.com");
  });

  /// The tray is 328px wide. The description used to share a nowrap row with
  /// the raw URL, which ate the width and ellipsised the provider's own
  /// wording down to a few characters.
  it("keeps the whole description in the DOM, however long", () => {
    const wording =
      "Elevated error rates on the API and increased latency for a subset of requests";
    render(
      <MenuCard
        provider={provider()}
        hideEmail={false}
        resetTimeRelative
        incident={{ ...incident, description: wording }}
      />,
    );

    expect(screen.getByText(wording)).toBeInTheDocument();
    // The URL is on the control, not spending row width as plain text.
    expect(screen.queryByText("https://status.openai.com")).not.toBeInTheDocument();
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
