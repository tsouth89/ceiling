import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getLocaleStrings: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
}));

vi.mock("../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/tauri")>()),
  ...tauriMocks,
}));
vi.mock("@tauri-apps/api/event", () => eventMocks);

import { LocaleProvider } from "../i18n/LocaleProvider";
import { buildBundle } from "../test/localeHarness";
import { CopyIconButton } from "./MenuCard";

describe("CopyIconButton", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        PanelCopied: "Copied",
        ActionCopyError: "Copy error",
        ProviderIssueCopy: "Copy",
      }),
    );
    eventMocks.listen.mockResolvedValue(() => {});
  });

  it("exposes the neutral copy label in the idle state", async () => {
    render(
      <LocaleProvider>
        <CopyIconButton text="hello" />
      </LocaleProvider>,
    );

    const btn = await screen.findByRole("button", { name: "Copy" });
    expect(btn).toBeInTheDocument();
    expect(btn).toHaveAttribute("title", "Copy");
  });

  it("handles resolved clipboard promise and displays success state", async () => {
    const writeTextSpy = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", {
      clipboard: {
        writeText: writeTextSpy,
      },
    });

    render(
      <LocaleProvider>
        <CopyIconButton text="test-copy" />
      </LocaleProvider>,
    );

    const btn = await screen.findByRole("button", { name: "Copy" });
    fireEvent.click(btn);

    expect(writeTextSpy).toHaveBeenCalledWith("test-copy");

    // Success state should show "Copied"
    const successBtn = await screen.findByRole("button", { name: "Copied" });
    expect(successBtn).toBeInTheDocument();
    expect(successBtn).toHaveTextContent("✓");

    // Wait for the timeout to revert back to idle
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
    }, { timeout: 1500 });

    vi.unstubAllGlobals();
  });

  it("handles rejected clipboard promise and displays failure state", async () => {
    const writeTextSpy = vi.fn().mockRejectedValue(new Error("Clipboard access denied"));
    vi.stubGlobal("navigator", {
      clipboard: {
        writeText: writeTextSpy,
      },
    });

    render(
      <LocaleProvider>
        <CopyIconButton text="test-fail" />
      </LocaleProvider>,
    );

    const btn = await screen.findByRole("button", { name: "Copy" });
    fireEvent.click(btn);

    expect(writeTextSpy).toHaveBeenCalledWith("test-fail");

    // Failure state should show "Copy error"
    const failureBtn = await screen.findByRole("button", { name: "Copy error" });
    expect(failureBtn).toBeInTheDocument();

    // Wait for the timeout to revert back to idle
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
    }, { timeout: 1500 });

    vi.unstubAllGlobals();
  });
});
