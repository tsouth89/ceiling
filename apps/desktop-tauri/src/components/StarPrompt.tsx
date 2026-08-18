import { useEffect, useRef } from "react";
import { useLocale } from "../hooks/useLocale";
import type { StarPromptReason } from "../lib/starPrompt";

interface Props {
  reason: StarPromptReason;
  /** App version, shown in the copy for the post-update ask. */
  version: string;
  onStar: () => void;
  onDismiss: () => void;
}

/**
 * The GitHub star ask (SOU-311). A card in the bottom-right of the dashboard,
 * shown at most twice in the app's life; `starPrompt.ts` owns when.
 *
 * Deliberately not a modal: no backdrop, no focus trap, and focus is left
 * where the user put it. It sits over the corner of a panel someone opened to
 * read a number, and taking the keyboard away from them to ask a favour would
 * be the rudest possible version of this feature. It stays until it is
 * answered because a nudge that fades out unread would only have to be shown
 * again, which is exactly the repetition the two-ask cap exists to avoid.
 */
export default function StarPrompt({
  reason,
  version,
  onStar,
  onDismiss,
}: Props) {
  const { t } = useLocale();
  const onDismissRef = useRef(onDismiss);
  onDismissRef.current = onDismiss;

  // Escape is the expected way out of anything that floats above the page, and
  // it counts as "Later" — the same as the close button, and never as a star.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onDismissRef.current();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const title =
    reason === "afterUpdate"
      ? `${t("StarPromptTitleUpdated")} ${version}`
      : t("StarPromptTitleRunning");

  return (
    <aside
      className="star-prompt"
      role="dialog"
      aria-label={t("StarPromptAriaLabel")}
    >
      <div className="star-prompt__header">
        <span className="star-prompt__check" aria-hidden="true" />
        <h3 className="star-prompt__title">{title}</h3>
        <button
          type="button"
          className="star-prompt__close"
          onClick={onDismiss}
          aria-label={t("StarPromptLater")}
          title={t("StarPromptLater")}
        >
          ✕
        </button>
      </div>
      <p className="star-prompt__body">{t("StarPromptBody")}</p>
      <div className="star-prompt__actions">
        <button
          type="button"
          className="star-prompt__action star-prompt__action--primary"
          onClick={onStar}
        >
          <span className="star-prompt__star" aria-hidden="true">
            ☆
          </span>
          {t("StarPromptStar")}
        </button>
        <button
          type="button"
          className="star-prompt__action"
          onClick={onDismiss}
        >
          {t("StarPromptLater")}
        </button>
      </div>
    </aside>
  );
}
