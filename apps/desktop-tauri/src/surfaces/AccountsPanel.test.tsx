import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import AccountsPanel from "./AccountsPanel";
import type { ProviderUsageSnapshot } from "../types/bridge";

vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({
    t: (k: string) => (k === "UpdatedJustNow" ? "just now" : k),
  }),
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
    providerId: "claude",
    displayName: "Claude",
    primary: win,
    primaryLabel: "Weekly",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    inactiveRateWindows: [],
    cost: null,
    planName: "Claude Max 5x",
    accountEmail: "user@example.com",
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

describe("AccountsPanel", () => {
  it("renders one card per provider with plan and account", () => {
    const { container, getByText } = render(
      <AccountsPanel
        providers={[
          provider(),
          provider({
            providerId: "codex",
            displayName: "Codex",
            planName: "ChatGPT Plus",
            accountEmail: "codex@example.com",
          }),
        ]}
        hideEmail={false}
        onManage={() => {}}
      />,
    );
    expect(container.querySelectorAll(".account-card")).toHaveLength(2);
    expect(getByText("Claude")).toBeTruthy();
    expect(getByText("Claude Max 5x")).toBeTruthy();
    expect(getByText("user@example.com")).toBeTruthy();
  });

  it("marks a healthy provider connected and an errored one as error", () => {
    const { container } = render(
      <AccountsPanel
        providers={[
          provider({ providerId: "claude", displayName: "Claude" }),
          provider({
            providerId: "codex",
            displayName: "Codex",
            error: "network timeout",
          }),
        ]}
        hideEmail={false}
        onManage={() => {}}
      />,
    );
    const dots = [...container.querySelectorAll(".account-card__dot")];
    const statuses = dots.map((d) => d.getAttribute("data-status"));
    expect(statuses).toContain("connected");
    expect(statuses).toContain("error");
    // The error message surfaces on the card.
    expect(container.textContent).toContain("network timeout");
  });

  it("masks the email when hidePersonalInfo is on", () => {
    const { container } = render(
      <AccountsPanel
        providers={[provider({ accountEmail: "user@example.com" })]}
        hideEmail
        onManage={() => {}}
      />,
    );
    expect(container.textContent).not.toContain("user@example.com");
    expect(container.textContent).toContain("u•••");
  });

  it("invokes onManage when a card or the footer link is clicked", () => {
    const onManage = vi.fn();
    const { container, getByText } = render(
      <AccountsPanel providers={[provider()]} hideEmail={false} onManage={onManage} />,
    );
    fireEvent.click(container.querySelector(".account-card") as HTMLElement);
    fireEvent.click(getByText("Add or manage providers"));
    expect(onManage).toHaveBeenCalledTimes(2);
  });

  it("shows an empty state with a manage link when there are no providers", () => {
    const onManage = vi.fn();
    const { getByText } = render(
      <AccountsPanel providers={[]} hideEmail={false} onManage={onManage} />,
    );
    expect(getByText("No accounts yet")).toBeTruthy();
    fireEvent.click(getByText("Manage providers"));
    expect(onManage).toHaveBeenCalledTimes(1);
  });
});
