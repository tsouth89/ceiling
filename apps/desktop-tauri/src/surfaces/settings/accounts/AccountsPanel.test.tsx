import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderAccountsBridge } from "../../../types/bridge";
import { AccountsPanel } from "./AccountsPanel";

const tauriMocks = vi.hoisted(() => ({
  getDirectoryAccounts: vi.fn(),
  probeAccountDirectory: vi.fn(),
  addDirectoryAccount: vi.fn(),
  removeDirectoryAccount: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => tauriMocks);
vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({
    t: (key: string) => key,
  }),
}));

function provider(
  overrides: Partial<ProviderAccountsBridge> = {},
): ProviderAccountsBridge {
  return {
    providerId: "codex",
    displayName: "Codex",
    envVar: "CODEX_HOME",
    accounts: [],
    activeIndex: 0,
    followingCli: true,
    ambientDir: "C:\\codex",
    ...overrides,
  };
}

describe("settings AccountsPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows the followed path and a -work setup suggestion when home is resolved", async () => {
    tauriMocks.getDirectoryAccounts.mockResolvedValue([provider()]);

    const { container } = render(<AccountsPanel />);

    await waitFor(() => {
      expect(container.querySelector(".accounts-path")?.textContent).toContain(
        "C:\\codex",
      );
    });
    expect(container.textContent).toContain("C:\\codex-work");
    expect(screen.getByPlaceholderText("C:\\codex-work")).toBeTruthy();
  });

  it("does not treat a missing ambient dir as a path", async () => {
    tauriMocks.getDirectoryAccounts.mockResolvedValue([
      provider({ ambientDir: null }),
    ]);

    const { container } = render(<AccountsPanel />);

    await waitFor(() => {
      expect(container.querySelector(".accounts-following")).toBeTruthy();
    });

    expect(container.textContent).not.toContain("-work");
    for (const path of container.querySelectorAll("code.accounts-path")) {
      expect(path.textContent?.trim()).not.toBe("");
    }
    expect(container.textContent).toContain("$env:CODEX_HOME");
    expect(screen.getByPlaceholderText("AccountsDirPlaceholder")).toBeTruthy();
  });

  it("does not treat an empty ambient dir as a path", async () => {
    tauriMocks.getDirectoryAccounts.mockResolvedValue([
      provider({ ambientDir: "   " }),
    ]);

    const { container } = render(<AccountsPanel />);

    await waitFor(() => {
      expect(container.querySelector(".accounts-following")).toBeTruthy();
    });

    expect(container.textContent).not.toContain("-work");
    for (const path of container.querySelectorAll("code.accounts-path")) {
      expect(path.textContent?.trim()).not.toBe("");
    }
  });
});
