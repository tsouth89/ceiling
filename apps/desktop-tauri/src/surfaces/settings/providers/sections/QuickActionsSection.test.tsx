import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LocaleProvider } from "../../../../i18n/LocaleProvider";
import { buildBundle } from "../../../../test/localeHarness";
import type { ProviderDetail } from "../../../../types/bridge";
import { QuickActionsSection } from "./QuickActionsSection";

const tauriMocks = vi.hoisted(() => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
}));

vi.mock("../../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../../lib/tauri")>()),
  ...tauriMocks,
}));
vi.mock("@tauri-apps/api/event", () => eventMocks);

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
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getLocaleStrings.mockResolvedValue(buildBundle());
    eventMocks.listen.mockResolvedValue(() => {});
  });

  it("shows the device-flow code alongside the login status", async () => {
    render(
      <LocaleProvider>
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
        />
      </LocaleProvider>,
    );

    expect(await screen.findByRole("status")).toHaveTextContent(
      "LoginPhaseWaitingBrowser LoginPhaseEnterGithubCodePrefix ABCD-1234",
    );
  });

  it("does not render a code during waitingBrowser when none is carried yet", async () => {
    render(
      <LocaleProvider>
        <QuickActionsSection
          provider={provider()}
          busy={false}
          loginPhase="waitingBrowser"
          loginCode={null}
          onRefresh={noop}
          onSwitchAccount={noop}
          onOpenDashboard={noop}
          onOpenStatusPage={noop}
          onCopyError={noop}
          onBuyCredits={noop}
          t={(key) => key}
        />
      </LocaleProvider>,
    );

    expect(await screen.findByRole("status")).not.toHaveTextContent(
      "LoginPhaseEnterGithubCodePrefix",
    );
  });

  it("does not render a stale code once the phase moves past waitingBrowser", async () => {
    render(
      <LocaleProvider>
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
        />
      </LocaleProvider>,
    );

    await screen.findByRole("status");
    expect(screen.queryByText("ABCD-1234")).not.toBeInTheDocument();
  });
});
