/**
 * Masking for "Hide Personal Info".
 *
 * One implementation on purpose. This lived twice, in `MenuCard` and
 * `AccountsPanel`, with different signatures and different output, and most
 * surfaces used neither: the setting was on while the Overview, taskbar
 * flyout, plan cards, activity timeline and provider detail all printed the
 * raw address.
 */

/** Matches an address anywhere in a string, so labels containing one are caught. */
const EMAIL_PATTERN = /[^\s@]+@[^\s@.]+\.[^\s@]+/g;

/**
 * Mask a single address: first character, then dots, then the domain.
 *
 * The domain survives because it is what makes an account recognisable to its
 * owner ("the cssi one") without naming the person to anyone watching.
 */
export function maskEmail(email: string): string {
  const at = email.indexOf("@");
  if (at <= 1) return "••••@••••";
  return email[0] + "•".repeat(at - 1) + email.slice(at);
}

/**
 * Mask every address inside arbitrary text, leaving the rest intact.
 *
 * Account labels are auto-derived as `email (plan)` when the user does not type
 * their own, so masking a bare `accountEmail` field is not enough: the label is
 * a second copy of the same address. This masks in place, so
 * `bts@cssi.us (max)` becomes `b••@cssi.us (max)` and a hand-typed "Work"
 * passes through untouched.
 */
export function maskEmailsIn(text: string): string {
  return text.replace(EMAIL_PATTERN, (match) => maskEmail(match));
}

/** Apply {@link maskEmailsIn} only when the setting is on. Null passes through. */
export function maskIdentity(
  text: string | null | undefined,
  hide: boolean,
): string | null {
  if (!text) return null;
  return hide ? maskEmailsIn(text) : text;
}
