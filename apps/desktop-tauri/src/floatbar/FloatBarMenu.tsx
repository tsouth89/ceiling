import type { MouseEvent, ReactNode } from "react";

/**
 * Right-click action row for the floating bar. The floatbar window auto-sizes
 * to its content, so a floating popup menu would be clipped by the tiny window;
 * instead this renders IN PLACE of the pills (same in-flow row, so the window
 * just resizes to fit) as a compact strip of labeled actions. It replaces the
 * webview's generic browser context menu with purposeful controls.
 */

function MenuIcon({ children }: { children: ReactNode }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="13"
      height="13"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      {children}
    </svg>
  );
}

export default function FloatBarMenu({
  locked,
  clickThrough,
  onToggleLock,
  onToggleClickThrough,
  onOpenSettings,
  onHide,
}: {
  locked: boolean;
  clickThrough: boolean;
  onToggleLock: () => void;
  onToggleClickThrough: () => void;
  onOpenSettings: () => void;
  onHide: () => void;
}) {
  // Stop mousedown from reaching the bar's drag handler / native drag region so
  // clicking an action doesn't start a window drag instead.
  const stop = (event: MouseEvent) => event.stopPropagation();

  return (
    <div
      className="floatbar__menu"
      role="menu"
      aria-label="Floating bar actions"
      onMouseDown={stop}
    >
      <button
        type="button"
        role="menuitemcheckbox"
        aria-checked={locked}
        className={`floatbar__menu-item${locked ? " floatbar__menu-item--active" : ""}`}
        title={locked ? "Unlock — allow dragging" : "Lock in place"}
        onMouseDown={stop}
        onClick={onToggleLock}
      >
        <MenuIcon>
          <rect x="5" y="11" width="14" height="10" rx="2" />
          {locked ? (
            <path d="M8 11V7a4 4 0 0 1 8 0v4" />
          ) : (
            <path d="M8 11V7a4 4 0 0 1 7.8-1.2" />
          )}
        </MenuIcon>
        <span>{locked ? "Unlock" : "Lock"}</span>
      </button>

      <button
        type="button"
        role="menuitemcheckbox"
        aria-checked={clickThrough}
        className={`floatbar__menu-item${clickThrough ? " floatbar__menu-item--active" : ""}`}
        title="Click-through — let clicks pass to the desktop (turn off in Settings)"
        onMouseDown={stop}
        onClick={onToggleClickThrough}
      >
        <MenuIcon>
          <path d="M5 3l5.5 15 2.2-5.8L18 10z" />
        </MenuIcon>
        <span>Click-through</span>
      </button>

      <button
        type="button"
        role="menuitem"
        className="floatbar__menu-item"
        title="Open Ceiling settings"
        onMouseDown={stop}
        onClick={onOpenSettings}
      >
        <MenuIcon>
          <circle cx="12" cy="12" r="3" />
          <path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M16.9 16.9l2.1 2.1M19.1 4.9l-2.1 2.1M7 16.9l-2.1 2.1" />
        </MenuIcon>
        <span>Settings</span>
      </button>

      <button
        type="button"
        role="menuitem"
        className="floatbar__menu-item"
        title="Hide the floating bar (re-enable it in Settings)"
        onMouseDown={stop}
        onClick={onHide}
      >
        <MenuIcon>
          <path d="M6 9l6 6 6-6" />
          <path d="M4 20h16" />
        </MenuIcon>
        <span>Hide</span>
      </button>
    </div>
  );
}
