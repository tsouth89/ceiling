import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { buildBundle } from "../../test/localeHarness";

vi.mock("../../lib/tauri", () => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

import { LocaleProvider } from "../../i18n/LocaleProvider";
import { useResetCountdown } from "./useResetCountdown";
import * as tauri from "../../lib/tauri";

function Probe({
  resetsAt,
  fallback,
}: {
  resetsAt: string | null;
  fallback: string | null;
}) {
  const text = useResetCountdown(resetsAt, fallback);
  return <span data-testid="cd">{text ?? "null"}</span>;
}

async function mountWithLocale(ui: React.ReactNode) {
  // Resolve the bundle synchronously so <LocaleProvider> doesn't suspend.
  (tauri.getLocaleStrings as ReturnType<typeof vi.fn>).mockResolvedValue(
    buildBundle({
      ResetsInHoursMinutes: "{}h {}m",
      ResetsInMinutes: "{}m",
      ResetsInDaysHours: "{}d {}h",
      TrayResetsDueNow: "resetting",
    }),
  );
  const rendered = render(<LocaleProvider>{ui}</LocaleProvider>);
  // Flush the pending bundle fetch.
  await act(async () => {});
  return rendered;
}

describe("useResetCountdown", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-06-01T00:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns the fallback when `resetsAt` is null", async () => {
    await mountWithLocale(<Probe resetsAt={null} fallback="in 3h" />);
    expect(screen.getByTestId("cd")).toHaveTextContent("in 3h");
  });

  it("returns the fallback when the timestamp is unparseable", async () => {
    await mountWithLocale(<Probe resetsAt="not-a-date" fallback="later" />);
    expect(screen.getByTestId("cd")).toHaveTextContent("later");
  });

  it("renders hours+minutes for sub-day deltas", async () => {
    const target = new Date("2024-06-01T03:42:00Z").toISOString();
    await mountWithLocale(<Probe resetsAt={target} fallback="x" />);
    expect(screen.getByTestId("cd")).toHaveTextContent("3h 42m");
  });

  it("omits zero hours for sub-hour deltas", async () => {
    const target = new Date("2024-06-01T00:40:00Z").toISOString();
    await mountWithLocale(<Probe resetsAt={target} fallback="x" />);
    expect(screen.getByTestId("cd")).toHaveTextContent(/^40m$/);
  });

  it("renders days+hours for multi-day deltas", async () => {
    const target = new Date("2024-06-03T05:00:00Z").toISOString();
    await mountWithLocale(<Probe resetsAt={target} fallback="x" />);
    expect(screen.getByTestId("cd")).toHaveTextContent("2d 5h");
  });

  it("reports 'resetting' when the target is already in the past", async () => {
    const past = new Date("2024-05-31T23:00:00Z").toISOString();
    await mountWithLocale(<Probe resetsAt={past} fallback="x" />);
    expect(screen.getByTestId("cd")).toHaveTextContent("resetting");
  });

  it("re-renders on its 30-second tick", async () => {
    const target = new Date("2024-06-01T02:00:00Z").toISOString();
    await mountWithLocale(<Probe resetsAt={target} fallback="x" />);
    expect(screen.getByTestId("cd")).toHaveTextContent("2h 0m");

    await act(async () => {
      vi.advanceTimersByTime(60 * 60_000); // +1h
    });
    expect(screen.getByTestId("cd")).toHaveTextContent("1h 0m");
  });
});
