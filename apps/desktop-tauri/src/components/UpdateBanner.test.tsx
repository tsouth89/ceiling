import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { UpdateStatePayload } from "../types/bridge";
import UpdateBanner from "./UpdateBanner";
import { expectNoAccessibilityViolations } from "../test/accessibility";

vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

const handlers = {
  onCheck: vi.fn(),
  onDownload: vi.fn(),
  onApply: vi.fn(),
  onDismiss: vi.fn(),
  onOpenRelease: vi.fn(),
};

function state(
  status: UpdateStatePayload["status"],
  overrides: Partial<UpdateStatePayload> = {},
): UpdateStatePayload {
  return {
    status,
    version: "2.0.0",
    error: null,
    progress: null,
    releaseUrl: null,
    canDownload: true,
    canApply: true,
    lastCheckedAt: null,
    ...overrides,
  };
}

describe("UpdateBanner accessibility", () => {
  it("announces available and ready states as status updates", () => {
    const { rerender } = render(
      <UpdateBanner updateState={state("available")} {...handlers} />,
    );
    expect(screen.getByRole("status").textContent).toContain("2.0.0");

    rerender(<UpdateBanner updateState={state("ready")} {...handlers} />);
    expect(screen.getByRole("status").textContent).toContain(
      "BannerReadyToInstallSuffix",
    );
  });

  it("exposes download progress without changing the status announcement", () => {
    const { rerender } = render(
      <UpdateBanner
        updateState={state("downloading", { progress: 0.42 })}
        {...handlers}
      />,
    );
    const status = screen.getByRole("status");
    expect(status.textContent).not.toContain("42%");
    expect(screen.getByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "42",
    );

    rerender(
      <UpdateBanner
        updateState={state("downloading", { progress: 0.73 })}
        {...handlers}
      />,
    );
    expect(screen.getByRole("status").textContent).toBe(status.textContent);
    expect(screen.getByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "73",
    );
  });

  it("announces update failures as alerts", () => {
    render(
      <UpdateBanner
        updateState={state("error", { error: "network failed" })}
        {...handlers}
      />,
    );
    expect(screen.getByRole("alert").textContent).toContain("network failed");
  });

  it("has no detectable accessibility violations", async () => {
    const { container } = render(
      <UpdateBanner updateState={state("available")} {...handlers} />,
    );

    await expectNoAccessibilityViolations(container);
  });
});
