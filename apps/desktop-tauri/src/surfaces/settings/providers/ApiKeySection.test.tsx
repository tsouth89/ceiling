import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiKeySection } from "./ApiKeySection";

const tauriMocks = vi.hoisted(() => ({
  getApiKeyProviders: vi.fn(),
  getApiKeys: vi.fn(),
  removeApiKey: vi.fn(),
  setApiKey: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => tauriMocks);
vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({
    t: (key: string) =>
      key === "ConfirmRemoveApiKeyBody" ? "Remove the saved API key for {}?" : key,
  }),
}));

describe("ApiKeySection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getApiKeyProviders.mockResolvedValue([
      {
        id: "claude",
        displayName: "Claude",
        envVar: null,
        help: null,
        dashboardUrl: null,
      },
    ]);
    tauriMocks.getApiKeys.mockResolvedValue([]);
    tauriMocks.setApiKey.mockResolvedValue([
      {
        providerId: "claude",
        provider: "Claude",
        maskedKey: "sk-...test",
        savedAt: "now",
        label: null,
      },
    ]);
    tauriMocks.removeApiKey.mockResolvedValue([]);
  });

  it("notifies the detail pane after a key is saved", async () => {
    const onCredentialsChanged = vi.fn();
    render(
      <ApiKeySection
        providerId="claude"
        onCredentialsChanged={onCredentialsChanged}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Add Key" }));
    fireEvent.change(screen.getByPlaceholderText("Paste API key…"), {
      target: { value: "sk-test" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(onCredentialsChanged).toHaveBeenCalledOnce());
    expect(tauriMocks.setApiKey).toHaveBeenCalledWith(
      "claude",
      "sk-test",
      undefined,
    );
  });

  it("cancels API-key removal without calling the backend", async () => {
    tauriMocks.getApiKeys.mockResolvedValue([
      {
        providerId: "claude",
        provider: "Claude",
        maskedKey: "sk-...test",
        savedAt: "now",
        label: null,
      },
    ]);
    const onCredentialsChanged = vi.fn();
    render(
      <ApiKeySection
        providerId="claude"
        onCredentialsChanged={onCredentialsChanged}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    fireEvent.click(screen.getByRole("button", { name: "ConfirmCancel" }));
    expect(tauriMocks.removeApiKey).not.toHaveBeenCalled();
    expect(onCredentialsChanged).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("removes an API key only after confirmation and reports success", async () => {
    tauriMocks.getApiKeys.mockResolvedValue([
      {
        providerId: "claude",
        provider: "Claude",
        maskedKey: "sk-...test",
        savedAt: "now",
        label: null,
      },
    ]);
    const onCredentialsChanged = vi.fn();
    render(
      <ApiKeySection
        providerId="claude"
        onCredentialsChanged={onCredentialsChanged}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    expect(screen.getByRole("alertdialog")).toHaveTextContent("Claude");
    fireEvent.click(screen.getByRole("button", { name: "ConfirmRemove" }));

    await waitFor(() => expect(onCredentialsChanged).toHaveBeenCalledOnce());
    expect(tauriMocks.removeApiKey).toHaveBeenCalledWith("claude");
    expect(screen.getByRole("status")).toHaveTextContent("CredentialRemoved");
  });

  it("keeps the key and shows an error when removal fails", async () => {
    tauriMocks.getApiKeys.mockResolvedValue([
      {
        providerId: "claude",
        provider: "Claude",
        maskedKey: "sk-...test",
        savedAt: "now",
        label: null,
      },
    ]);
    tauriMocks.removeApiKey.mockRejectedValue(new Error("store locked"));
    render(<ApiKeySection providerId="claude" />);

    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    fireEvent.click(screen.getByRole("button", { name: "ConfirmRemove" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("store locked");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByText("sk-...test")).toBeInTheDocument();
  });
});
