import { describe, expect, it } from "vitest";
import { TAB_META } from "../surfaces/Settings";
import rustSource from "../../src-tauri/src/surface_target.rs?raw";
import bridgeSource from "../types/bridge.ts?raw";
import floatBarSource from "../floatbar/FloatBar.tsx?raw";
import popOutSource from "../surfaces/PopOutPanel.tsx?raw";
import traySource from "../surfaces/TrayPanel.tsx?raw";

function quoted(block: string): string[] {
  return [...block.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

function rustTabs(src: string): string[] {
  const block = src.match(/const SETTINGS_TAB_IDS: &\[&str\] = &\[([\s\S]*?)\];/);
  if (!block) throw new Error("SETTINGS_TAB_IDS not found");
  return quoted(block[1]);
}

function unionTabs(src: string): string[] {
  const block = src.match(/export type SettingsTabId =\s*([\s\S]*?);/);
  if (!block) throw new Error("SettingsTabId not found");
  return quoted(block[1]);
}

function callTabs(src: string): string[] {
  return [...src.matchAll(/openSettingsWindow\(\s*["]([^"]+)["]\s*\)/g)].map(
    (match) => match[1],
  );
}

describe("settings tab contract (SBS-872)", () => {
  const live = TAB_META.map((tab) => tab.id);

  it("keeps the Rust allowlist identical to TAB_META", () => {
    expect(rustTabs(rustSource)).toEqual(live);
  });

  it("keeps SettingsTabId identical to TAB_META", () => {
    expect(unionTabs(bridgeSource)).toEqual(live);
  });

  it("makes every openSettingsWindow caller send a live tab", () => {
    const calls = [
      ...callTabs(floatBarSource).map((tab) => ({ file: "FloatBar.tsx", tab })),
      ...callTabs(popOutSource).map((tab) => ({ file: "PopOutPanel.tsx", tab })),
      ...callTabs(traySource).map((tab) => ({ file: "TrayPanel.tsx", tab })),
    ];
    expect(calls.length).toBeGreaterThan(0);
    for (const call of calls) {
      expect(live, call.file + " sent " + call.tab).toContain(call.tab);
    }
  });

  it("opens Display from the float bar via the live menu id", () => {
    expect(callTabs(floatBarSource)).toEqual(["menu"]);
  });
});
