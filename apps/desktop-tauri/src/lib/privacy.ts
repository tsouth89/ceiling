/**
 * Masking for "Hide Personal Info".
 *
 * One implementation on purpose. This lived twice, in `MenuCard` and
 * `AccountsPanel`, with different signatures and different output, and most
 * surfaces used neither: the setting was on while the Overview, taskbar
 * flyout, plan cards, activity timeline and provider detail all printed the
 * raw address.
 */

/**
 * Matches an address anywhere in a string, so labels containing one are caught.
 *
 * The local part is the set legal in an unquoted address, which has to be got
 * right in both directions.
 *
 * Too permissive ("anything that is not a space") swallows adjacent label
 * text: `Work:bts@cssi.us` matched from `Work:bts` and masked to
 * `W•••••••@cssi.us`, destroying the part the user typed to tell the account
 * apart. Labels are hand-written, so punctuation runs straight into the
 * address with no space to stop it.
 *
 * Too narrow leaks the address it is meant to hide: omitting `'` matched
 * `o'connor@example.com` only from `connor`, leaving `o'` on screen. So the
 * class carries the full unquoted set, which still excludes the separators
 * (`:`, whitespace, brackets, commas) that appear in labels.
 */
const EMAIL_PATTERN =
  /[A-Za-z0-9!#$%&'*+/=?^_`{|}~.-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+/g;

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
