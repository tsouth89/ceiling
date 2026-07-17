import type { LocaleKey } from "../i18n/keys";

const DISMISS_KEY = "ceiling.first-run.dismissed.v1";

/** Whether the user has dismissed the first-run checklist on this machine. */
export function firstRunDismissed(): boolean {
  try {
    return localStorage.getItem(DISMISS_KEY) === "1";
  } catch {
    return false;
  }
}

/** Remember that the first-run checklist was dismissed. */
export function dismissFirstRun(): void {
  try {
    localStorage.setItem(DISMISS_KEY, "1");
  } catch {
    // Ignore storage failures (private mode, disabled storage, etc.).
  }
}

interface Props {
  enabledCount: number;
  hasWorkingAuth: boolean;
  floatbarEnabled: boolean;
  onOpenProviders: () => void;
  onOpenDisplay: () => void;
  onDismiss: () => void;
  t: (key: LocaleKey) => string;
}

/**
 * Light, dismissible first-run checklist shown in the empty dashboard when no
 * provider has produced usage yet (SOU-157). Each step reflects real state and
 * deep-links into the matching Settings tab. Dismissal is remembered in
 * localStorage (same pattern as the detected-accounts card); a returning user
 * who already has working auth never lands here because the dashboard shows
 * their providers instead of the empty state.
 */
export function FirstRunChecklist({
  enabledCount,
  hasWorkingAuth,
  floatbarEnabled,
  onOpenProviders,
  onOpenDisplay,
  onDismiss,
  t,
}: Props) {
  const steps = [
    {
      done: enabledCount > 0,
      label: t("FirstRunStepEnable"),
      action: t("FirstRunOpenProviders"),
      onAction: onOpenProviders,
    },
    {
      done: hasWorkingAuth,
      label: t("FirstRunStepAuth"),
      action: t("FirstRunOpenProviders"),
      onAction: onOpenProviders,
    },
    {
      done: floatbarEnabled,
      label: t("FirstRunStepFloatbar"),
      action: t("FirstRunOpenDisplay"),
      onAction: onOpenDisplay,
    },
  ];

  return (
    <section className="first-run" aria-label={t("FirstRunTitle")}>
      <div className="first-run__header">
        <h3 className="first-run__title">{t("FirstRunTitle")}</h3>
        <button
          type="button"
          className="first-run__dismiss"
          onClick={onDismiss}
        >
          {t("FirstRunDismiss")}
        </button>
      </div>
      <ol className="first-run__steps">
        {steps.map((step, index) => (
          <li
            key={index}
            className={`first-run__step${step.done ? " first-run__step--done" : ""}`}
          >
            <span className="first-run__check" aria-hidden="true" />
            <span className="first-run__label">{step.label}</span>
            {!step.done && (
              <button
                type="button"
                className="first-run__action"
                onClick={step.onAction}
              >
                {step.action}
              </button>
            )}
          </li>
        ))}
      </ol>
    </section>
  );
}
