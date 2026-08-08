import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LocaleProvider } from "../../../i18n/LocaleProvider";
import { buildBundle } from "../../../test/localeHarness";
import type { ProviderTokenAccountsBridge } from "../../../types/bridge";
import { TokenAccountsPanel } from "./TokenAccountsPanel";

const tauriMocks = vi.hoisted(() => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
  getTokenAccounts: vi.fn(),
  triggerProviderLogin: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
  listeners: new Map<string, Array<(event: { payload: unknown }) => void>>(),
}));

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  ...tauriMocks,
}));
vi.mock("@tauri-apps/api/event", () => eventMocks);

function emptyAccounts(): ProviderTokenAccountsBridge {
  return {
    providerId: "copilot",
    support: {
      providerId: "copilot",
      displayName: "GitHub Copilot",
      title: "Copilot",
      subtitle: "",
      placeholder: "Paste token…",
    },
    accounts: [],
    activeIndex: -1,
  };
}

function emitLoginPhaseChanged(payload: unknown) {
  for (const listener of eventMocks.listeners.get("login-phase-changed") ?? []) {
    listener({ payload });
  }
}

describe("TokenAccountsPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    eventMocks.listeners.clear();
    tauriMocks.getLocaleStrings.mockResolvedValue(buildBundle());
    tauriMocks.getTokenAccounts.mockResolvedValue(emptyAccounts());
    tauriMocks.triggerProviderLogin.mockResolvedValue(undefined);
    eventMocks.listen.mockImplementation(
      (event: string, handler: (event: { payload: unknown }) => void) => {
        const listeners = eventMocks.listeners.get(event) ?? [];
        listeners.push(handler);
        eventMocks.listeners.set(event, listeners);
        return Promise.resolve(() => {});
      },
    );
  });

  it("shows the device-flow code alongside the waiting-for-browser status", async () => {
    render(
      <LocaleProvider>
        <TokenAccountsPanel providerId="copilot" />
      </LocaleProvider>,
    );

    await screen.findByText("TokenAccountGithubLoginButton");

    act(() => {
      emitLoginPhaseChanged({
        providerId: "copilot",
        phase: "waitingBrowser",
        code: "ABCD-1234",
      });
    });

    expect(await screen.findByText("ABCD-1234")).toBeInTheDocument();
  });

  it("does not render a code when the phase carries none", async () => {
    render(
      <LocaleProvider>
        <TokenAccountsPanel providerId="copilot" />
      </LocaleProvider>,
    );

    await screen.findByText("TokenAccountGithubLoginButton");

    act(() => {
      emitLoginPhaseChanged({ providerId: "copilot", phase: "requesting" });
    });

    expect(screen.queryByText("ABCD-1234")).not.toBeInTheDocument();
  });

  it("does not render a stale code once the phase moves past waitingBrowser", async () => {
    render(
      <LocaleProvider>
        <TokenAccountsPanel providerId="copilot" />
      </LocaleProvider>,
    );

    await screen.findByText("TokenAccountGithubLoginButton");

    act(() => {
      emitLoginPhaseChanged({
        providerId: "copilot",
        phase: "waitingBrowser",
        code: "ABCD-1234",
      });
    });
    expect(await screen.findByText("ABCD-1234")).toBeInTheDocument();

    // A phase change that (incorrectly) still carries the old code must not
    // keep it on screen — the render is gated on the phase, not just the code.
    act(() => {
      emitLoginPhaseChanged({
        providerId: "copilot",
        phase: "complete",
        code: "ABCD-1234",
      });
    });
    await waitFor(() =>
      expect(screen.queryByText("ABCD-1234")).not.toBeInTheDocument(),
    );
  });
});
