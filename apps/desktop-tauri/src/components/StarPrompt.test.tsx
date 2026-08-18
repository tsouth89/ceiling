import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({ listen: vi.fn() }));

vi.mock("../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/tauri")>()),
  ...tauriMocks,
}));
vi.mock("@tauri-apps/api/event", () => eventMocks);

import { LocaleProvider } from "../i18n/LocaleProvider";
import { buildBundle } from "../test/localeHarness";
import StarPrompt from "./StarPrompt";
import type { StarPromptReason } from "../lib/starPrompt";

function renderPrompt(
  reason: StarPromptReason,
  handlers: { onStar?: () => void; onDismiss?: () => void } = {},
) {
  const onStar = handlers.onStar ?? vi.fn();
  const onDismiss = handlers.onDismiss ?? vi.fn();
  render(
    <LocaleProvider>
      <StarPrompt
        reason={reason}
        version="1.5.34"
        onStar={onStar}
        onDismiss={onDismiss}
      />
    </LocaleProvider>,
  );
  return { onStar, onDismiss };
}

describe("StarPrompt", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    eventMocks.listen.mockResolvedValue(() => {});
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        StarPromptAriaLabel: "Star Ceiling on GitHub",
        StarPromptTitleRunning: "Ceiling is up and running",
        StarPromptTitleUpdated: "Updated to",
        StarPromptBody:
          "If Ceiling is useful to you, a GitHub star helps other developers find it.",
        StarPromptStar: "Star on GitHub",
        StarPromptLater: "Later",
      }),
    );
  });

  it("leads with the app working, not with the ask", async () => {
    renderPrompt("firstValue");
    expect(
      await screen.findByText("Ceiling is up and running"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "If Ceiling is useful to you, a GitHub star helps other developers find it.",
      ),
    ).toBeInTheDocument();
  });

  it("names the version on the post-update ask", async () => {
    renderPrompt("afterUpdate");
    expect(await screen.findByText("Updated to 1.5.34")).toBeInTheDocument();
  });

  it("reports the star, not a dismissal, when the user goes to GitHub", async () => {
    const { onStar, onDismiss } = renderPrompt("firstValue");
    fireEvent.click(await screen.findByRole("button", { name: "Star on GitHub" }));
    expect(onStar).toHaveBeenCalledOnce();
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it("treats the close button as Later, never as a star", async () => {
    const { onStar, onDismiss } = renderPrompt("firstValue");
    const buttons = await screen.findAllByRole("button", { name: "Later" });
    // Both the ✕ and the Later button carry the same accessible name; either
    // one must dismiss without being counted as interest.
    for (const button of buttons) fireEvent.click(button);
    expect(onDismiss).toHaveBeenCalledTimes(buttons.length);
    expect(onStar).not.toHaveBeenCalled();
  });

  it("dismisses on Escape", async () => {
    const { onStar, onDismiss } = renderPrompt("firstValue");
    await screen.findByRole("dialog", { name: "Star Ceiling on GitHub" });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onDismiss).toHaveBeenCalledOnce();
    expect(onStar).not.toHaveBeenCalled();
  });

  it("does not take focus from whatever the user was reading", async () => {
    const before = document.activeElement;
    renderPrompt("firstValue");
    await screen.findByRole("dialog", { name: "Star Ceiling on GitHub" });
    expect(document.activeElement).toBe(before);
  });
});
