import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LocaleProvider } from "../../../i18n/LocaleProvider";
import { buildBundle } from "../../../test/localeHarness";
import type { ProviderTokenAccountsBridge } from "../../../types/bridge";
import { TokenAccountsPanel } from "./TokenAccountsPanel";

const tauriMocks = vi.hoisted(() => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
  getTokenAccounts: vi.fn(),
  removeTokenAccount: vi.fn(),
  triggerProviderLogin: vi.fn(),
  openExternalUrl: vi.fn(),
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

async function waitForLoginPhaseListener() {
  await waitFor(() =>
    expect(eventMocks.listeners.get("login-phase-changed")).toHaveLength(1),
  );
}

describe("TokenAccountsPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    eventMocks.listeners.clear();
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        ConfirmRemoveTokenBody: "Remove the {} token account from {}?",
      }),
    );
    tauriMocks.getTokenAccounts.mockResolvedValue(emptyAccounts());
    tauriMocks.removeTokenAccount.mockResolvedValue(emptyAccounts());
    tauriMocks.triggerProviderLogin.mockResolvedValue(undefined);
    tauriMocks.openExternalUrl.mockResolvedValue(undefined);
    eventMocks.listen.mockImplementation(
      (event: string, handler: (event: { payload: unknown }) => void) => {
        const listeners = eventMocks.listeners.get(event) ?? [];
        listeners.push(handler);
        eventMocks.listeners.set(event, listeners);
        return Promise.resolve(() => {});
      },
    );
  });

  it("shows the device-flow code and verification link alongside the waiting-for-browser status", async () => {
    render(
      <LocaleProvider>
        <TokenAccountsPanel providerId="copilot" />
      </LocaleProvider>,
    );

    await screen.findByText("TokenAccountGithubLoginButton");
    await waitForLoginPhaseListener();

    act(() => {
      emitLoginPhaseChanged({
        providerId: "copilot",
        phase: "waitingBrowser",
        code: "ABCD-1234",
        url: "https://github.com/login/device",
      });
    });

    expect(await screen.findByText("ABCD-1234")).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "LoginPhaseOpenVerificationLink" }),
    );
    expect(tauriMocks.openExternalUrl).toHaveBeenCalledWith(
      "https://github.com/login/device",
    );
  });

  it("shows an error when opening the verification link fails", async () => {
    tauriMocks.openExternalUrl.mockRejectedValue(new Error("no browser"));

    render(
      <LocaleProvider>
        <TokenAccountsPanel providerId="copilot" />
      </LocaleProvider>,
    );

    await screen.findByText("TokenAccountGithubLoginButton");
    await waitForLoginPhaseListener();

    act(() => {
      emitLoginPhaseChanged({
        providerId: "copilot",
        phase: "waitingBrowser",
        code: "ABCD-1234",
        url: "https://github.com/login/device",
      });
    });

    fireEvent.click(
      await screen.findByRole("button", {
        name: "LoginPhaseOpenVerificationLink",
      }),
    );

    expect(await screen.findByText("no browser")).toBeInTheDocument();
  });

  it("clears the link error once a retry succeeds", async () => {
    tauriMocks.openExternalUrl.mockRejectedValueOnce(new Error("no browser"));
    tauriMocks.openExternalUrl.mockResolvedValueOnce(undefined);

    render(
      <LocaleProvider>
        <TokenAccountsPanel providerId="copilot" />
      </LocaleProvider>,
    );

    await screen.findByText("TokenAccountGithubLoginButton");
    await waitForLoginPhaseListener();
    act(() => {
      emitLoginPhaseChanged({
        providerId: "copilot",
        phase: "waitingBrowser",
        code: "ABCD-1234",
        url: "https://github.com/login/device",
      });
    });

    const link = await screen.findByRole("button", {
      name: "LoginPhaseOpenVerificationLink",
    });
    fireEvent.click(link);
    expect(await screen.findByText("no browser")).toBeInTheDocument();

    fireEvent.click(link);
    await waitFor(() =>
      expect(screen.queryByText("no browser")).not.toBeInTheDocument(),
    );
  });

  it("clears a stale link error once a new login attempt starts", async () => {
    tauriMocks.openExternalUrl.mockRejectedValue(new Error("no browser"));

    render(
      <LocaleProvider>
        <TokenAccountsPanel providerId="copilot" />
      </LocaleProvider>,
    );

    await screen.findByText("TokenAccountGithubLoginButton");
    await waitForLoginPhaseListener();
    act(() => {
      emitLoginPhaseChanged({
        providerId: "copilot",
        phase: "waitingBrowser",
        code: "ABCD-1234",
        url: "https://github.com/login/device",
      });
    });
    fireEvent.click(
      await screen.findByRole("button", {
        name: "LoginPhaseOpenVerificationLink",
      }),
    );
    expect(await screen.findByText("no browser")).toBeInTheDocument();

    // Same URL as before — GitHub's plain verification_uri is a constant,
    // so the clear must be keyed on the code, not the URL, to catch this.
    act(() => {
      emitLoginPhaseChanged({
        providerId: "copilot",
        phase: "waitingBrowser",
        code: "WXYZ-5678",
        url: "https://github.com/login/device",
      });
    });
    await waitFor(() =>
      expect(screen.queryByText("no browser")).not.toBeInTheDocument(),
    );
  });

  it("does not render a code when the phase carries none", async () => {
    render(
      <LocaleProvider>
        <TokenAccountsPanel providerId="copilot" />
      </LocaleProvider>,
    );

    await screen.findByText("TokenAccountGithubLoginButton");
    await waitForLoginPhaseListener();

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
    await waitForLoginPhaseListener();

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

  it("masks the token field and requires confirmation before removing an account", async () => {
    const onCredentialsChanged = vi.fn();
    tauriMocks.getTokenAccounts.mockResolvedValue({
      ...emptyAccounts(),
      accounts: [
        {
          id: "acct-1",
          label: "Work",
          addedAt: "today",
          lastUsed: null,
          isActive: true,
        },
      ],
      activeIndex: 0,
    });

    render(
      <LocaleProvider>
        <TokenAccountsPanel
          providerId="copilot"
          onCredentialsChanged={onCredentialsChanged}
        />
      </LocaleProvider>,
    );

    expect(await screen.findByLabelText("SecretFieldTokenLabel")).toHaveClass(
      "secret-field__input--masked",
    );
    fireEvent.click(screen.getByRole("button", { name: "TokenAccountRemove" }));
    fireEvent.click(screen.getByRole("button", { name: "ConfirmCancel" }));
    expect(tauriMocks.removeTokenAccount).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "TokenAccountRemove" }));
    expect(screen.getByRole("alertdialog")).toHaveTextContent("Work");
    fireEvent.click(screen.getByRole("button", { name: "ConfirmRemove" }));
    await waitFor(() => expect(onCredentialsChanged).toHaveBeenCalledOnce());
    expect(tauriMocks.removeTokenAccount).toHaveBeenCalledWith("copilot", "acct-1");
  });
});
