import { describe, expect, it } from "vitest";
import { TAB_META } from "./Settings";

/** Live Settings shell tabs, in render order. SBS-872. */
const LIVE_SETTINGS_TABS = [
  "general",
  "providers",
  "accounts",
  "notifications",
  "menu",
  "advanced",
  "about",
] as const;

describe("Settings navigation", () => {
  it("lists providers separately after general", () => {
    expect(TAB_META.slice(0, 2)).toEqual([
      { id: "general", labelKey: "TabGeneral" },
      { id: "providers", labelKey: "TabProviders" },
    ]);
  });

  it("exposes exactly the live shell tabs", () => {
    // Pins SBS-872: TAB_META is the only list isSettingsTab consults. A
    // caller that still sends a retired id (menuBar, apiKeys, display)
    // falls through to General. settingsTabs.test.ts compares this list
    // to the Rust allowlist and the SettingsTabId union.
    expect(TAB_META.map((tab) => tab.id)).toEqual([...LIVE_SETTINGS_TABS]);
  });
});
