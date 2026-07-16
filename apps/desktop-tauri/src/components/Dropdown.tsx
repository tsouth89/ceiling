import { useEffect, useId, useRef, useState } from "react";

export type DropdownOption = { value: string; label: string };

/**
 * A fully CSS-themed listbox dropdown that replaces the native `<select>`.
 *
 * Native `<select>` popups on Windows are drawn by WebView2 and do not reliably
 * honor the app's `color-scheme` or the Tauri window theme, so they render in
 * the OS theme (a light popup inside our dark UI). This control draws its own
 * popup, so it matches dark and light exactly (SOU-224). It implements the ARIA
 * listbox keyboard model: Up/Down/Home/End move the active option, Enter/Space
 * commit, Escape closes, and clicking outside dismisses.
 */
export function Dropdown({
  value,
  options,
  onChange,
  disabled,
  className,
  buttonClassName,
  ariaLabel,
  style,
}: {
  value: string;
  options: DropdownOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  className?: string;
  buttonClassName?: string;
  ariaLabel?: string;
  style?: React.CSSProperties;
}) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const baseId = useId();

  const selectedIndex = options.findIndex((option) => option.value === value);
  const selectedLabel = options[selectedIndex]?.label ?? value;

  // Dismiss on outside pointer press.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  // On open, focus the list and start on the selected option.
  useEffect(() => {
    if (!open) return;
    setActiveIndex(selectedIndex >= 0 ? selectedIndex : 0);
    listRef.current?.focus();
  }, [open, selectedIndex]);

  const commit = (index: number) => {
    const option = options[index];
    if (option) onChange(option.value);
    setOpen(false);
    buttonRef.current?.focus();
  };

  const onButtonKeyDown = (event: React.KeyboardEvent) => {
    if (disabled) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      setOpen(true);
    }
  };

  const onListKeyDown = (event: React.KeyboardEvent) => {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        setActiveIndex((index) => Math.min(options.length - 1, index + 1));
        break;
      case "ArrowUp":
        event.preventDefault();
        setActiveIndex((index) => Math.max(0, index - 1));
        break;
      case "Home":
        event.preventDefault();
        setActiveIndex(0);
        break;
      case "End":
        event.preventDefault();
        setActiveIndex(options.length - 1);
        break;
      case "Enter":
      case " ":
        event.preventDefault();
        commit(activeIndex);
        break;
      case "Escape":
        event.preventDefault();
        setOpen(false);
        buttonRef.current?.focus();
        break;
      case "Tab":
        setOpen(false);
        break;
      default:
        break;
    }
  };

  return (
    <div className={`dropdown${className ? ` ${className}` : ""}`} ref={rootRef} style={style}>
      <button
        type="button"
        ref={buttonRef}
        className={`dropdown__button${buttonClassName ? ` ${buttonClassName}` : ""}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        disabled={disabled}
        onClick={() => !disabled && setOpen((value) => !value)}
        onKeyDown={onButtonKeyDown}
      >
        <span className="dropdown__value">{selectedLabel}</span>
        <span className="dropdown__chevron" aria-hidden="true" />
      </button>
      {open && (
        <ul
          className="dropdown__list"
          role="listbox"
          ref={listRef}
          tabIndex={-1}
          aria-label={ariaLabel}
          aria-activedescendant={activeIndex >= 0 ? `${baseId}-option-${activeIndex}` : undefined}
          onKeyDown={onListKeyDown}
        >
          {options.map((option, index) => (
            <li
              key={option.value}
              id={`${baseId}-option-${index}`}
              role="option"
              aria-selected={option.value === value}
              className={`dropdown__option${index === activeIndex ? " dropdown__option--active" : ""}${
                option.value === value ? " dropdown__option--selected" : ""
              }`}
              onPointerEnter={() => setActiveIndex(index)}
              onClick={() => commit(index)}
            >
              {option.label}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
