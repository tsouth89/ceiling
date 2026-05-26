import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
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
import { MenuSummary } from "./MenuSurface";

function renderSummary(total: number) {
  return render(
    <LocaleProvider>
      <MenuSummary
        total={total}
        errorCount={0}
        isRefreshing={false}
        lastRefresh={null}
      />
    </LocaleProvider>,
  );
}

describe("MenuSummary", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({ SummaryProvidersLabel: "providers" }),
    );
    eventMocks.listen.mockResolvedValue(() => {});
  });

  it("uses a singular provider label for one provider", async () => {
    renderSummary(1);

    expect(await screen.findByText("1 provider")).toBeInTheDocument();
  });

  it("keeps the plural provider label for multiple providers", async () => {
    renderSummary(2);

    expect(await screen.findByText("2 providers")).toBeInTheDocument();
  });
});
