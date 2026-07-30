import { describe, expect, it } from "vitest";

/**
 * Mirrors the ordering in ProvidersTab. Kept as a pure function here so the
 * rule can be asserted without mounting the whole settings surface.
 */
function sortProviders<T extends { id: string; displayName: string }>(
  providers: T[],
  enabled: Set<string>,
): T[] {
  const withIndex = providers.map((provider, index) => ({ provider, index }));
  withIndex.sort((a, b) => {
    const aOn = enabled.has(a.provider.id);
    const bOn = enabled.has(b.provider.id);
    if (aOn !== bOn) return aOn ? -1 : 1;
    if (aOn) return a.index - b.index;
    return a.provider.displayName.localeCompare(b.provider.displayName);
  });
  return withIndex.map((entry) => entry.provider);
}

const catalog = [
  { id: "zeta", displayName: "Zeta" },
  { id: "claude", displayName: "Claude" },
  { id: "opencodego", displayName: "OpenCode Go" },
  { id: "alpha", displayName: "Alpha" },
  { id: "codex", displayName: "Codex" },
];

describe("providers sidebar ordering", () => {
  it("floats enabled providers to the top", () => {
    // The reported symptom: OpenCode Go was configured but buried far down.
    const sorted = sortProviders(catalog, new Set(["opencodego", "codex"]));
    expect(sorted.map((p) => p.id).slice(0, 2)).toEqual([
      "opencodego",
      "codex",
    ]);
  });

  it("keeps drag order among the enabled ones", () => {
    // provider_order also drives tray and pop-out card order, so alphabetising
    // the enabled group would silently undo a deliberate arrangement.
    const dragged = [
      { id: "codex", displayName: "Codex" },
      { id: "claude", displayName: "Claude" },
      { id: "alpha", displayName: "Alpha" },
    ];
    const sorted = sortProviders(dragged, new Set(["codex", "claude"]));
    expect(sorted.map((p) => p.id)).toEqual(["codex", "claude", "alpha"]);
  });

  it("sorts the disabled tail by name", () => {
    const sorted = sortProviders(catalog, new Set(["codex"]));
    expect(sorted.map((p) => p.displayName)).toEqual([
      "Codex",
      "Alpha",
      "Claude",
      "OpenCode Go",
      "Zeta",
    ]);
  });

  it("is stable, so re-sorting its own output changes nothing", () => {
    // The list re-sorts every render off persisted order; a rule that is not
    // idempotent would make rows drift on each refresh.
    const enabled = new Set(["opencodego", "codex"]);
    const once = sortProviders(catalog, enabled);
    expect(sortProviders(once, enabled)).toEqual(once);
  });
});
