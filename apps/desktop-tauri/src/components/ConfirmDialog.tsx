import { useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";

export function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel,
  cancelLabel,
  busy,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  body: string;
  confirmLabel: string;
  cancelLabel: string;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const titleId = useId();
  const bodyId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    cancelRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        event.preventDefault();
        onCancel();
        return;
      }
      if (event.key !== "Tab") return;
      const root = dialogRef.current;
      if (!root) return;
      const focusable = dialogFocusable(root);
      if (focusable.length === 0) return;
      event.preventDefault();
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const index = focusable.findIndex((element) => element === document.activeElement);
      if (event.shiftKey) {
        (index <= 0 ? last : focusable[index - 1]).focus();
      } else {
        (index === -1 || index === focusable.length - 1 ? first : focusable[index + 1]).focus();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [busy, onCancel, open]);

  if (!open) return null;

  return createPortal(
    <div className="confirm-dialog-backdrop" onClick={() => !busy && onCancel()}>
      <div
        ref={dialogRef}
        className="confirm-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={bodyId}
        onClick={(event) => event.stopPropagation()}
      >
        <h2 id={titleId} className="confirm-dialog__title">
          {title}
        </h2>
        <p id={bodyId} className="confirm-dialog__body">
          {body}
        </p>
        <div className="confirm-dialog__actions">
          <button
            ref={cancelRef}
            type="button"
            className="credential-btn"
            disabled={busy}
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
            type="button"
            className="credential-btn credential-btn--danger"
            disabled={busy}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

function dialogFocusable(root: HTMLElement): HTMLElement[] {
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ),
  );
}
