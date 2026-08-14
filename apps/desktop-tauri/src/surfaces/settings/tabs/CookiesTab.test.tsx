import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getManualCookies: vi.fn(),
  removeManualCookie: vi.fn(),
  setManualCookie: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => tauriMocks);
vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

import CookiesTab from "./CookiesTab";

describe("CookiesTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getManualCookies.mockResolvedValue([]);
  });

  it("shows the localized manual-cookie guide without browser import controls", async () => {
    render(<CookiesTab providers={[]} />);

    expect(await screen.findByText("BrowserCookieMigrationNotice")).toBeInTheDocument();
    expect(screen.getByText("BrowserCookiePasteGuideTitle")).toBeInTheDocument();
    expect(screen.getByText("BrowserCookiePasteGuideSignIn")).toBeInTheDocument();
    expect(screen.getByText("BrowserCookiePasteGuidePrivacy")).toBeInTheDocument();
    expect(screen.queryByText(/Import from Browser/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Import Cookies/i })).not.toBeInTheDocument();
  });

  it("masks the cookie field and confirms removal", async () => {
    tauriMocks.getManualCookies.mockResolvedValue([
      { providerId: "claude", provider: "Claude", savedAt: "now" },
    ]);
    tauriMocks.removeManualCookie.mockResolvedValue([]);

    render(<CookiesTab providers={[]} />);

    expect(await screen.findByLabelText("SecretFieldCookieLabel")).toHaveClass(
      "secret-field__input--masked",
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    fireEvent.click(screen.getByRole("button", { name: "ConfirmCancel" }));
    expect(tauriMocks.removeManualCookie).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    fireEvent.click(screen.getByRole("button", { name: "ConfirmRemove" }));
    await waitFor(() =>
      expect(tauriMocks.removeManualCookie).toHaveBeenCalledWith("claude"),
    );
  });
});
