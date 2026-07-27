import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import DetectedAccountsCard from "./DetectedAccountsCard";
import { setDetectedProviderIgnored } from "../lib/detectedProviderPreferences";

const getDetectedProviderAccounts = vi.fn();
const importCopilotFromGh = vi.fn();

vi.mock("../lib/tauri", () => ({
  getDetectedProviderAccounts: () => getDetectedProviderAccounts(),
  importCopilotFromGh: () => importCopilotFromGh(),
}));

describe("DetectedAccountsCard", () => {
  beforeEach(() => {
    window.localStorage.clear();
    getDetectedProviderAccounts.mockReset();
    importCopilotFromGh.mockReset();
    importCopilotFromGh.mockResolvedValue({
      providerId: "copilot",
      support: {
        providerId: "copilot",
        displayName: "Copilot",
        title: "GitHub accounts",
        subtitle: "",
        placeholder: "",
      },
      accounts: [],
      activeIndex: 0,
    });
    getDetectedProviderAccounts.mockResolvedValue([
      {
        providerId: "codex",
        displayName: "Codex",
        status: "ready",
        sourceLabel: "Codex CLI",
        detail: "Signed in and ready to track",
      },
      {
        providerId: "gemini",
        displayName: "Gemini",
        status: "ready",
        sourceLabel: "Gemini CLI",
        detail: "Signed in and ready to track",
      },
      {
        providerId: "cursor",
        displayName: "Cursor",
        status: "unavailable",
        sourceLabel: "Cursor for Windows",
        detail: "Not found on this PC",
      },
    ]);
  });

  it("pins current gh account when tracking Copilot", async () => {
    getDetectedProviderAccounts.mockResolvedValue([
      {
        providerId: "copilot",
        displayName: "Copilot",
        status: "ready",
        sourceLabel: "GitHub CLI (will pin on track)",
        detail: "Signed in and ready to track",
      },
    ]);
    const onEnable = vi.fn().mockResolvedValue(undefined);
    render(
      <DetectedAccountsCard
        enabledProviderIds={[]}
        onEnable={onEnable}
        onManage={vi.fn()}
      />,
    );
    await waitFor(() => expect(screen.getByText("Copilot")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Track" }));
    await waitFor(() => expect(importCopilotFromGh).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(onEnable).toHaveBeenCalledWith(["copilot"]));
  });

  it("offers one-click tracking only for detected disabled providers", async () => {
    const onEnable = vi.fn().mockResolvedValue(undefined);
    render(
      <DetectedAccountsCard
        enabledProviderIds={["codex"]}
        onEnable={onEnable}
        onManage={vi.fn()}
      />,
    );

    expect(await screen.findByText("Available to track")).toBeTruthy();
    expect(screen.queryByText("Codex CLI")).toBeNull();
    expect(screen.getByText("Gemini CLI")).toBeTruthy();
    expect(screen.queryByText("Cursor for Windows")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Track" }));
    await waitFor(() => expect(onEnable).toHaveBeenCalledWith(["gemini"]));
  });

  it("dismisses the current detection signature", async () => {
    const props = {
      enabledProviderIds: ["codex"],
      onEnable: vi.fn().mockResolvedValue(undefined),
      onManage: vi.fn(),
    };
    const first = render(<DetectedAccountsCard {...props} />);
    expect(await screen.findByText("Available to track")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByText("Available to track")).toBeNull();
    first.unmount();

    render(<DetectedAccountsCard {...props} />);
    await waitFor(() => expect(getDetectedProviderAccounts).toHaveBeenCalledTimes(2));
    expect(screen.queryByText("Available to track")).toBeNull();
  });

  it("does not re-suggest a provider the user explicitly disabled", async () => {
    setDetectedProviderIgnored("gemini", true);

    render(
      <DetectedAccountsCard
        enabledProviderIds={["codex"]}
        onEnable={vi.fn().mockResolvedValue(undefined)}
        onManage={vi.fn()}
      />,
    );

    await waitFor(() => expect(getDetectedProviderAccounts).toHaveBeenCalled());
    expect(screen.queryByText("Available to track")).toBeNull();
    expect(screen.queryByText("Gemini CLI")).toBeNull();
  });

  it("migrates a cached disabled provider into the persistent ignore list", async () => {
    const props = {
      enabledProviderIds: ["codex"],
      onEnable: vi.fn().mockResolvedValue(undefined),
      onManage: vi.fn(),
    };
    const first = render(
      <DetectedAccountsCard {...props} previouslyTrackedProviderIds={["gemini"]} />,
    );

    await waitFor(() => expect(getDetectedProviderAccounts).toHaveBeenCalled());
    expect(screen.queryByText("Available to track")).toBeNull();
    first.unmount();

    render(<DetectedAccountsCard {...props} />);
    await waitFor(() => expect(getDetectedProviderAccounts).toHaveBeenCalledTimes(2));
    expect(screen.queryByText("Available to track")).toBeNull();
  });
});
