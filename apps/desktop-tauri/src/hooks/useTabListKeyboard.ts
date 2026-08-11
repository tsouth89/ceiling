import { useId } from "react";

/**
 * Whether arrowing to a tab also selects it.
 *
 * WAI-ARIA Authoring Practices asks for automatic activation when the panel
 * appears without noticeable latency, and manual activation otherwise. A strip
 * whose panel triggers a provider fetch should be `"manual"`, so arrowing past
 * three accounts does not fire three network refreshes. Manual activation needs
 * no extra key handling here: the tabs are real `<button>` elements, so Enter
 * and Space already fire their `onClick`.
 */
export type TabListActivation = "automatic" | "manual";

/** Tabs are ordered left to right, so Left/Right move and Home/End jump. */
const MOVE_KEYS = new Set(["ArrowLeft", "ArrowRight", "Home", "End"]);

function enabledTabsIn(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>('[role="tab"]:not([disabled])'),
  );
}

/**
 * Keyboard behavior and ARIA wiring for a `role="tablist"` strip (#219).
 *
 * Gives a strip the two things the role promises and none of ours had: arrow
 * keys move between tabs, and the strip is a single Tab stop rather than one
 * per tab (roving `tabindex`). Focus wraps at both ends. It also pairs each tab
 * with its panel through `aria-controls` / `aria-labelledby`.
 *
 * Every caller renders only the selected panel, so `aria-controls` is set on
 * the selected tab alone — pointing it at an element that is not in the
 * document would be worse than omitting it.
 */
export function useTabListKeyboard<Id extends string>({
  tabIds,
  selectedId,
  onSelect,
  activation = "automatic",
}: {
  tabIds: readonly Id[];
  selectedId: Id | null | undefined;
  onSelect: (id: Id) => void;
  activation?: TabListActivation;
}) {
  const baseId = useId();

  // With nothing selected the first tab holds the strip's Tab stop, so the
  // strip stays reachable instead of dropping out of the tab order entirely.
  const selectedIndex = selectedId == null ? -1 : tabIds.indexOf(selectedId);
  const tabStopIndex = selectedIndex >= 0 ? selectedIndex : 0;

  const tabDomId = (index: number) => `${baseId}-tab-${index}`;
  const panelDomId = (index: number) => `${baseId}-panel-${index}`;

  const onKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    if (!MOVE_KEYS.has(event.key)) return;

    const tabs = enabledTabsIn(event.currentTarget);
    if (tabs.length === 0) return;

    // Read the origin off the event rather than `document.activeElement`: a
    // click-then-arrow in a real window and a `fireEvent.keyDown` in a test
    // then behave the same way.
    const origin = (event.target as HTMLElement | null)?.closest<HTMLElement>(
      '[role="tab"]',
    );
    const from = origin ? tabs.indexOf(origin) : -1;

    let next: number;
    switch (event.key) {
      case "ArrowRight":
        next = from < 0 ? 0 : (from + 1) % tabs.length;
        break;
      case "ArrowLeft":
        next = from < 0 ? tabs.length - 1 : (from - 1 + tabs.length) % tabs.length;
        break;
      case "Home":
        next = 0;
        break;
      default:
        next = tabs.length - 1;
        break;
    }

    event.preventDefault();
    const target = tabs[next];
    target.focus();
    if (activation === "automatic") {
      const id = target.dataset.tabId;
      if (id !== undefined) onSelect(id as Id);
    }
  };

  return {
    tabListProps: { role: "tablist" as const, onKeyDown },

    getTabProps: (id: Id) => {
      const index = tabIds.indexOf(id);
      const selected = index >= 0 && index === selectedIndex;
      return {
        role: "tab" as const,
        id: tabDomId(index),
        "data-tab-id": id,
        "aria-selected": selected,
        "aria-controls": selected ? panelDomId(index) : undefined,
        tabIndex: index === tabStopIndex ? 0 : -1,
      };
    },

    /**
     * Describes the panel of whichever tab is selected — callers render one
     * panel at a time, and deriving it here means the two ends of
     * `aria-controls` / `aria-labelledby` cannot drift apart.
     *
     * Spread this only where the strip itself is rendered: a panel naming a tab
     * that is not in the document is worse than a plain container. `tabIndex: 0`
     * is deliberate — it matches the APG tabs pattern, and several of these
     * panels are scroll containers a keyboard user otherwise cannot scroll.
     */
    getPanelProps: () => ({
      role: "tabpanel" as const,
      id: selectedIndex >= 0 ? panelDomId(selectedIndex) : undefined,
      "aria-labelledby": selectedIndex >= 0 ? tabDomId(selectedIndex) : undefined,
      tabIndex: 0,
    }),
  };
}
