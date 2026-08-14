import { useId, useState } from "react";

export function SecretField({
  label,
  value,
  onChange,
  placeholder,
  rows = 3,
  disabled,
  revealLabel,
  hideLabel,
  className,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  rows?: number;
  disabled?: boolean;
  revealLabel: string;
  hideLabel: string;
  className?: string;
}) {
  const fieldId = useId();
  const [revealed, setRevealed] = useState(false);

  return (
    <div className={`secret-field${className ? ` ${className}` : ""}`}>
      <div className="secret-field__header">
        <label className="secret-field__label" htmlFor={fieldId}>
          {label}
        </label>
        <button
          type="button"
          className="credential-btn credential-btn--secondary secret-field__reveal"
          aria-pressed={revealed}
          aria-controls={fieldId}
          disabled={disabled}
          onClick={() => setRevealed((current) => !current)}
        >
          {revealed ? hideLabel : revealLabel}
        </button>
      </div>
      <textarea
        id={fieldId}
        className={`text-input credential-textarea${revealed ? "" : " secret-field__input--masked"}`}
        value={value}
        placeholder={placeholder}
        rows={rows}
        disabled={disabled}
        autoComplete="off"
        autoCapitalize="none"
        autoCorrect="off"
        spellCheck={false}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}
