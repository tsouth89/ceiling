import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ActivityHeatmap, ActivityHourPoint } from "../types/bridge";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { buildBundle } from "../test/localeHarness";

const getLocalActivityHeatmap = vi.fn();
const getLocaleStrings = vi.fn();

vi.mock("../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/tauri")>()),
  getLocalActivityHeatmap: () => getLocalActivityHeatmap(),
  getLocaleStrings: () => getLocaleStrings(),
  setUiLanguage: vi.fn(),
}));

const eventMocks = vi.hoisted(() => {
  const handlers: Record<string, (event: { payload: unknown }) => void> = {};
  return {
    handlers,
    listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
      handlers[name] = handler;
      return Promise.resolve(() => {
        delete handlers[name];
      });
    }),
    emit(name: string, payload: unknown) {
      handlers[name]?.({ payload });
    },
  };
});

vi.mock("@tauri-apps/api/event", () => ({ listen: eventMocks.listen }));

const { ActivityHeatmapCard, formatActivityUsd, activityIntlLocale, localizedScanError } =
  await import(
  "./ActivityHeatmapCard"
);

const ENGLISH = {
  ActivityHeatmapTitle: "When you work",
  ActivityHeatmapSubtitle: "Last {} days of local activity, {}",
  ActivityHeatmapLocalTime: "local time",
  ActivityHeatmapProvidersHidden: "Every provider is hidden. Turn one back on above.",
  ActivityHeatmapEmpty: "No local activity in this window yet.",
  ActivityHeatmapTotal: "{} total",
  ActivityHeatmapDaysAria: "Daily activity for the last {} days",
  ActivityHeatmapCellSummary: "{}: {}, {} calls",
  ActivityHeatmapBusiest: "Busiest {}, around {}",
  ActivityHeatmapHourCell: "{} {}: {}, {} calls",
  ActivityHeatmapTokensUnit: "{} tokens",
};

function renderCard() {
  return render(
    <LocaleProvider>
      <ActivityHeatmapCard />
    </LocaleProvider>,
  );
}

const hour = (over: Partial<ActivityHourPoint> = {}): ActivityHourPoint => ({
  providerId: "codex",
  date: "2026-08-15",
  hour: 9,
  apiValueUsd: 4,
  tokens: 1000,
  calls: 2,
  ...over,
});

const heatmap = (over: Partial<ActivityHeatmap> = {}): ActivityHeatmap => ({
  days: ["2026-08-14", "2026-08-15"],
  providerIds: ["codex", "claude"],
  hours: [hour(), hour({ providerId: "claude", date: "2026-08-14", hour: 22 })],
  timezoneLabel: "UTC-07:00",
  ...over,
});

const activeDayCells = () =>
  [...document.querySelectorAll(".activity-card__cell--day")].filter(
    (cell) => cell.getAttribute("data-level") !== "0",
  ).length;

describe("ActivityHeatmapCard", () => {
  beforeEach(() => {
    getLocalActivityHeatmap.mockReset();
    getLocaleStrings.mockReset();
    getLocaleStrings.mockResolvedValue(buildBundle(ENGLISH));
  });

  it("draws a cell per day in range and lights the active ones", async () => {
    getLocalActivityHeatmap.mockResolvedValue(heatmap());
    renderCard();

    await waitFor(() => expect(screen.getByText("When you work")).toBeInTheDocument());
    expect(document.querySelectorAll(".activity-card__cell--day")).toHaveLength(2);
    expect(activeDayCells()).toBe(2);
    // The clock the buckets use is disclosed, not assumed.
    expect(screen.getByText(/UTC-07:00/)).toBeInTheDocument();
  });

  it("drops a provider's hours when its chip is switched off", async () => {
    getLocalActivityHeatmap.mockResolvedValue(heatmap());
    renderCard();
    await waitFor(() => expect(activeDayCells()).toBe(2));

    fireEvent.click(screen.getByRole("button", { name: /Claude/ }));

    await waitFor(() => expect(activeDayCells()).toBe(1));
  });

  /// Regression: an empty visible set used to mean "no filter", so turning
  /// every chip off showed every provider instead of an empty grid.
  it("empties the grid when every provider is switched off", async () => {
    getLocalActivityHeatmap.mockResolvedValue(heatmap());
    renderCard();
    await waitFor(() => expect(activeDayCells()).toBe(2));

    fireEvent.click(screen.getByRole("button", { name: /Claude/ }));
    fireEvent.click(screen.getByRole("button", { name: /Codex/ }));

    await waitFor(() => expect(activeDayCells()).toBe(0));
    // And it says why the grid is empty, rather than blaming the machine.
    expect(screen.getByText(/Every provider is hidden/)).toBeInTheDocument();
  });

  it("says the window is empty when there is genuinely no activity", async () => {
    getLocalActivityHeatmap.mockResolvedValue(
      heatmap({ providerIds: [], hours: [] }),
    );
    renderCard();

    await waitFor(() =>
      expect(screen.getByText(/No local activity in this window yet/)).toBeInTheDocument(),
    );
  });

  it("picks up a background rescan without being remounted", async () => {
    getLocalActivityHeatmap.mockResolvedValueOnce(heatmap());
    renderCard();
    await waitFor(() => expect(getLocalActivityHeatmap).toHaveBeenCalledTimes(1));

    getLocalActivityHeatmap.mockResolvedValueOnce(heatmap());
    act(() => {
      eventMocks.emit("local-scan-refreshed", "activity-heatmap");
    });

    await waitFor(() => expect(getLocalActivityHeatmap).toHaveBeenCalledTimes(2));
  });

  it("surfaces a failed read instead of drawing an empty grid", async () => {
    getLocalActivityHeatmap.mockRejectedValue(new Error("scan died"));
    renderCard();

    await waitFor(() => expect(screen.getByText("scan died")).toBeInTheDocument());
    expect(document.querySelectorAll(".activity-card__cell--day")).toHaveLength(0);
  });

  /// SBS-945: 29 quiet days and one spike used to share data-level 2, so the
  /// calendar painted the busiest day the same shade as the quietest.
  it("paints the spike day at the legend's top swatch", async () => {
    const days = Array.from({ length: 30 }, (_, index) => {
      const day = String(index + 1).padStart(2, "0");
      return `2026-08-${day}`;
    });
    const hours = days.map((date, index) =>
      hour({
        date,
        apiValueUsd: index === 29 ? 500 : 1,
        tokens: 1,
        calls: 1,
        providerId: "codex",
      }),
    );
    getLocalActivityHeatmap.mockResolvedValue(
      heatmap({ days, hours, providerIds: ["codex"] }),
    );
    renderCard();

    await waitFor(() =>
      expect(document.querySelectorAll(".activity-card__cell--day")).toHaveLength(30),
    );
    const levels = [...document.querySelectorAll(".activity-card__cell--day")].map((cell) =>
      cell.getAttribute("data-level"),
    );
    expect(levels[0]).toBe("2");
    expect(levels[28]).toBe("2");
    expect(levels[29]).toBe("4");
  });

  /// Failure mode: the card never imported useLocale, so every title, empty
  /// state, and aria-label stayed English after Language was set to Chinese
  /// (SBS-972).
  it("renders the locale bundle instead of hardcoded English", async () => {
    getLocaleStrings.mockResolvedValue(
      buildBundle(
        {
          ActivityHeatmapTitle: "工作时段",
          ActivityHeatmapReading: "正在读取本地活动…",
          ActivityHeatmapByDay: "按天",
          ActivityHeatmapByHour: "按小时",
          ActivityHeatmapLess: "少",
          ActivityHeatmapMore: "多",
          ActivityHeatmapNoPeak: "还没有高峰",
        },
        "chinese",
      ),
    );
    getLocalActivityHeatmap.mockResolvedValue(heatmap());
    renderCard();

    await waitFor(() => expect(screen.getByText("工作时段")).toBeInTheDocument());
    expect(screen.queryByText("When you work")).not.toBeInTheDocument();
    expect(screen.getByText("按天")).toBeInTheDocument();
    expect(screen.getByText("按小时")).toBeInTheDocument();
    expect(screen.getByText("少")).toBeInTheDocument();
    expect(screen.getByText("多")).toBeInTheDocument();
  });

  /// Failure mode: formatUsd/formatTokens pinned Intl.NumberFormat to the
  /// literal "en-US", so a Chinese UI language still printed $1,234.50 and
  /// 12.3K (SBS-972).
  it("formats dollars with the resolved locale, not a hardcoded en-US", () => {
    expect(activityIntlLocale("chinese")).toBe("zh-CN");
    expect(activityIntlLocale("english")).toBe("en-US");
    // SBS-972: anything without its own bundle renders English copy, so it
    // must format in English too. undefined handed Intl the OS locale, so a
    // Japanese selection on a German machine showed English words with
    // German dates and separators.
    for (const language of ["japanese", "spanish", "korean"] as const) {
      expect(activityIntlLocale(language as never)).toBe("en-US");
    }
    expect(formatActivityUsd(1234.5, "en-US")).toBe("$1,234.50");
    expect(formatActivityUsd(1234.5, "zh-CN")).toMatch(/US\$/);
    expect(formatActivityUsd(1234.5, "zh-CN")).not.toBe(
      formatActivityUsd(1234.5, "en-US"),
    );
  });

  /// SBS-972: the backend hands the failure back as an ordinary error string,
  /// so `tauriErrorMessage(err) || t(...)` never reached the localized copy —
  /// the sentinel is not empty, so it always won and a Chinese UI read English.
  it("translates the scan-failure sentinel but keeps real detail", () => {
    const readFailed = "无法读取本地活动。";
    expect(localizedScanError("Unable to read local activity.", readFailed)).toBe(readFailed);
    expect(localizedScanError("", readFailed)).toBe(readFailed);
    // A real error carries detail worth showing, so it passes through.
    expect(localizedScanError("permission denied: C:\\logs", readFailed)).toBe(
      "permission denied: C:\\logs",
    );
  });

  /// SBS-972: `t` was a dependency of the fetch effect, so every language
  /// switch re-ran the scan — the painted grid was replaced by the Reading
  /// spinner, and a cold cache repeated the whole transcript walk.
  it("does not refetch the heatmap when the language changes", async () => {
    getLocalActivityHeatmap.mockResolvedValue(heatmap({ providerIds: ["codex"] }));
    renderCard();

    await waitFor(() =>
      expect(document.querySelectorAll(".activity-card__cell--day").length).toBeGreaterThan(0),
    );
    expect(getLocalActivityHeatmap).toHaveBeenCalledTimes(1);

    getLocaleStrings.mockResolvedValue(
      buildBundle({ ActivityHeatmapTitle: "工作时段" }, "chinese"),
    );
    await act(async () => {
      eventMocks.handlers["locale-changed"]?.({ payload: { language: "chinese" } });
    });

    expect(
      getLocalActivityHeatmap,
      "a language switch must not re-run the transcript scan",
    ).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll(".activity-card__cell--day").length).toBeGreaterThan(0);
  });

});
