import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { buildBundle } from "../test/localeHarness";
import { useFormattedResetTime } from "./useFormattedResetTime";
import * as tauri from "../lib/tauri";

vi.mock("../lib/tauri", () => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

function Probe({
  resetsAt,
  fallback,
  relative,
}: {
  resetsAt: string | null;
  fallback: string | null;
  relative: boolean;
}) {
  const text = useFormattedResetTime(resetsAt, fallback, relative);
  return <span data-testid="reset">{text ?? "null"}</span>;
}

async function mountWithLocale(ui: React.ReactNode) {
  (tauri.getLocaleStrings as ReturnType<typeof vi.fn>).mockResolvedValue(
    buildBundle({
      MetricResetsIn: "Resets in",
      ResetsInHoursMinutes: "Resets in {}h {}m",
      ResetsInMinutes: "Resets in {}m",
      ResetsInDaysHours: "Resets in {}d {}h",
      ResetsInDays: "Resets in {}d",
      TrayResetsDueNow: "Resetting",
    }),
  );
  const rendered = render(<LocaleProvider>{ui}</LocaleProvider>);
  await act(async () => {});
  return rendered;
}

describe("useFormattedResetTime", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-06-01T00:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns a complete localized countdown in relative mode", async () => {
    const target = new Date("2024-06-01T03:42:00Z").toISOString();
    await mountWithLocale(
      <Probe resetsAt={target} fallback="later" relative={true} />,
    );
    expect(screen.getByTestId("reset")).toHaveTextContent("Resets in 3h 42m");
  });

  it("omits zero hours for sub-hour resets", async () => {
    const target = new Date("2024-06-01T00:40:00Z").toISOString();
    await mountWithLocale(
      <Probe resetsAt={target} fallback="later" relative={true} />,
    );
    expect(screen.getByTestId("reset")).toHaveTextContent("Resets in 40m");
  });

  it("omits zero hours for exact-day resets", async () => {
    const target = new Date("2024-06-25T00:00:00Z").toISOString();
    await mountWithLocale(
      <Probe resetsAt={target} fallback="later" relative={true} />,
    );
    expect(screen.getByTestId("reset")).toHaveTextContent("Resets in 24d");
  });

  it("leaves fallback text unlabelled in relative mode", async () => {
    await mountWithLocale(
      <Probe resetsAt={null} fallback="3h" relative={true} />,
    );
    expect(screen.getByTestId("reset")).toHaveTextContent("3h");
  });

  it("returns an absolute local time without the reset label", async () => {
    const target = new Date("2024-06-01T03:42:00Z").toISOString();
    await mountWithLocale(
      <Probe resetsAt={target} fallback="later" relative={false} />,
    );
    expect(screen.getByTestId("reset")).not.toHaveTextContent("Resets in");
  });
});
