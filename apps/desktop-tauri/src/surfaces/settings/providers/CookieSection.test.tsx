import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CookieSection } from "./CookieSection";

const tauriMocks = vi.hoisted(() => ({
  getManualCookies: vi.fn(),
  removeManualCookie: vi.fn(),
  setManualCookie: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => tauriMocks);
vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({
    t: (key: string) =>
      key === "ConfirmRemoveCookieBody" ? "Remove the saved cookie for {}?" : key,
  }),
}));

describe("CookieSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getManualCookies.mockResolvedValue([
      {
        providerId: "claude",
        provider: "Claude",
        savedAt: "now",
      },
    ]);
    tauriMocks.removeManualCookie.mockResolvedValue([]);
  });

  it("masks the cookie field by default", async () => {
    render(
      <CookieSection
        providerId="claude"
        providerName="Claude"
        cookieDomain="claude.ai"
      />,
    );

    const field = await screen.findByLabelText("SecretFieldCookieLabel");
    expect(field).toHaveClass("secret-field__input--masked");
  });

  it("does not remove a cookie when confirmation is cancelled", async () => {
    const onCredentialsChanged = vi.fn();
    render(
      <CookieSection
        providerId="claude"
        providerName="Claude"
        cookieDomain="claude.ai"
        onCredentialsChanged={onCredentialsChanged}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "BrowserCookieRemove" }));
    fireEvent.click(screen.getByRole("button", { name: "ConfirmCancel" }));
    expect(tauriMocks.removeManualCookie).not.toHaveBeenCalled();
    expect(onCredentialsChanged).not.toHaveBeenCalled();
  });

  it("removes a cookie after confirmation", async () => {
    const onCredentialsChanged = vi.fn();
    render(
      <CookieSection
        providerId="claude"
        providerName="Claude"
        cookieDomain="claude.ai"
        onCredentialsChanged={onCredentialsChanged}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "BrowserCookieRemove" }));
    expect(screen.getByRole("alertdialog")).toHaveTextContent("Claude");
    fireEvent.click(screen.getByRole("button", { name: "ConfirmRemove" }));
    await waitFor(() => expect(onCredentialsChanged).toHaveBeenCalledOnce());
    expect(tauriMocks.removeManualCookie).toHaveBeenCalledWith("claude");
    expect(screen.getByRole("status")).toHaveTextContent("CredentialRemoved");
  });
});
