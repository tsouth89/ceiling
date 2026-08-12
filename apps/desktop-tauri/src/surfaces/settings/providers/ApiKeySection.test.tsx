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
});
