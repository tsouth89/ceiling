import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderDetail } from "../../../../types/bridge";
import { QuickActionsSection } from "./QuickActionsSection";

function provider(): ProviderDetail {
  return {
    id: "copilot",
    displayName: "GitHub Copilot",
    enabled: true,
    email: null,
    plan: null,
    authType: null,
    sourceLabel: null,
    organization: null,
    lastUpdated: null,
    session: null,
    weekly: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    pace: null,
    lastError: null,
    dashboardUrl: null,
    statusPageUrl: null,
    buyCreditsUrl: null,
    hasSnapshot: false,
    cookieSource: null,
    region: null,
  };
}

const noop = vi.fn();

describe("QuickActionsSection", () => {
  it("shows the device-flow code alongside the login status", () => {
    render(
      <QuickActionsSection
        provider={provider()}
        busy={false}
        loginPhase="waitingBrowser"
        loginCode="ABCD-1234"
        onRefresh={noop}
        onSwitchAccount={noop}
        onOpenDashboard={noop}
        onOpenStatusPage={noop}
        onCopyError={noop}
        onBuyCredits={noop}
        t={(key) => key}
      />,
    );

    expect(screen.getByRole("status")).toHaveTextContent(
      "LoginPhaseWaitingBrowser LoginPhaseEnterGithubCodePrefix ABCD-1234",
    );
  });

  it("does not render a code when the phase carries none", () => {
    render(
      <QuickActionsSection
        provider={provider()}
        busy={false}
        loginPhase="requesting"
        loginCode={null}
        onRefresh={noop}
        onSwitchAccount={noop}
        onOpenDashboard={noop}
        onOpenStatusPage={noop}
        onCopyError={noop}
        onBuyCredits={noop}
        t={(key) => key}
      />,
    );

    expect(screen.queryByText("ABCD-1234")).not.toBeInTheDocument();
  });

  it("does not render a stale code once the phase moves past waitingBrowser", () => {
    render(
      <QuickActionsSection
        provider={provider()}
        busy={false}
        loginPhase="complete"
        loginCode="ABCD-1234"
        onRefresh={noop}
        onSwitchAccount={noop}
        onOpenDashboard={noop}
        onOpenStatusPage={noop}
        onCopyError={noop}
        onBuyCredits={noop}
        t={(key) => key}
      />,
    );

    expect(screen.queryByText("ABCD-1234")).not.toBeInTheDocument();
  });
});
