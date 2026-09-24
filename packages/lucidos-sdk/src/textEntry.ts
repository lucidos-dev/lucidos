/**
 * The text-entry contract: which fields take keyboard attributes, and what the
 * device's Autocorrect switch resolves to.
 *
 * Two stamps read it. The host stamps its own fields (`utils/noAutofill.ts`),
 * and the SDK stamps an app frame's fields (`autocorrectStamp.ts`). One copy is
 * what keeps an app's notes field and the host's composer on the same rule.
 *
 * **This module is pure**, like `appearance.ts`, and for the same reason it is
 * not re-exported from `index.ts`: the host reaches it through the
 * `@lucidos/text-entry` alias without pulling the SDK barrel into its graph.
 */

/** Input types that are not text entry, so no keyboard attribute applies.
 *  Allow-by-exclusion, so a new text-ish type (`date`, `month`, …) is covered
 *  without a list edit. */
export const NON_TEXT_INPUT_TYPES: ReadonlySet<string> = new Set([
  'button', 'submit', 'reset', 'image', 'file',
  'checkbox', 'radio', 'range', 'color', 'hidden',
]);

/** Whether `el` is a field the user types text into. */
export function isTextEntryField(el: Element): el is HTMLInputElement | HTMLTextAreaElement {
  // `localName`, not `instanceof`: an element class belongs to one realm, and
  // this module serves the host and every app frame.
  if (el.localName === 'textarea') return true;
  if (el.localName === 'input') return !NON_TEXT_INPUT_TYPES.has((el as HTMLInputElement).type);
  return false;
}

/** The shell's device-local mirror of the switch (workspace-scoped). The host
 *  writes it; an app frame that can read the shell's storage reads it before
 *  its preferences arrive. */
export const AUTOCORRECT_STORAGE_KEY = 'lucidos-autocorrect';

/** What an unset switch means: on, on every client. It is the one function
 *  that decides the default, so the host and every app frame agree. A device
 *  whose autocorrect keeps the tap on Send turns the switch off (ADR 0262). */
export function defaultAutocorrect(): boolean {
  return true;
}

/** Resolve the stored `autocorrect` preference. `'true'` and `'false'` win on
 *  any client; anything else is unset and falls to {@link defaultAutocorrect}. */
export function resolveAutocorrect(raw: string | null | undefined): boolean {
  if (raw === 'true') return true;
  if (raw === 'false') return false;
  return defaultAutocorrect();
}
