import type React from "react";

// ── tiny reusable controls ──────────────────────────────────────────

export function Toggle({
  checked,
  onChange,
  label,
  ariaLabel,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
  ariaLabel?: string;
  disabled?: boolean;
}) {
  const input = (
    <input
      type="checkbox"
      className="toggle"
      checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onChange={(e) => onChange(e.target.checked)}
    />
  );
  if (label) {
    return (
      <label className={`toggle-label ${disabled ? "toggle-label--disabled" : ""}`}>
        {input}
        <span>{label}</span>
      </label>
    );
  }
  return input;
}

export function Select({
  value,
  options,
  onChange,
  disabled,
}: {
  value: string;
  options: { value: string; label: string }[];
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  // Size for the longest option, not only the selected value. Native select
  // popovers use their longest label, so a narrow closed control can otherwise
  // open past the window edge. Count CJK glyphs as double-width and reserve a
  // dedicated arrow gutter so the label can never sit underneath the chevron.
  const labelUnits = (label: string) =>
    Array.from(label).reduce(
      (total, character) => total + (/[^\u0000-\u00ff]/.test(character) ? 2 : 1),
      0,
    );
  const longestLabelUnits = Math.max(
    labelUnits(value),
    ...options.map((option) => labelUnits(option.label)),
  );
  const width = Math.min(190, Math.max(78, Math.ceil(longestLabelUnits * 7.2) + 34));

  return (
    <select
      className="select"
      style={{ width }}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value)}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

export function NumberInput({
  value,
  min,
  max,
  step,
  onChange,
  disabled,
}: {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  onChange: (v: number) => void;
  disabled?: boolean;
}) {
  return (
    <input
      type="number"
      className="number-input"
      value={value}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      onChange={(e) => {
        const n = Number(e.target.value);
        if (!Number.isNaN(n)) onChange(n);
      }}
    />
  );
}

export function TextInput({
  value,
  placeholder,
  onChange,
  disabled,
}: {
  value: string;
  placeholder?: string;
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  return (
    <input
      type="text"
      className="text-input"
      value={value}
      placeholder={placeholder}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}

// ── field row ────────────────────────────────────────────────────────

export function Field({
  label,
  description,
  children,
  leading,
}: {
  label: string;
  description?: string;
  children: React.ReactNode;
  leading?: boolean;
}) {
  return (
    <div className={`settings-field${leading ? " settings-field--leading" : ""}`}>
      {leading && <div className="settings-field__control">{children}</div>}
      <div className="settings-field__text">
        <span className="settings-field__label">{label}</span>
        {description && (
          <span className="settings-field__desc">{description}</span>
        )}
      </div>
      {!leading && <div className="settings-field__control">{children}</div>}
    </div>
  );
}
