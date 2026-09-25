/**
 * The text-entry contract: which fields take keyboard attributes, what the
 * device's Autocorrect switch resolves to, and which insertions are key codes
 * rather than text.
 *
 * Two stamps read it. The host stamps its own fields (`utils/noAutofill.ts`),
 * and the SDK stamps an app frame's fields (`autocorrectStamp.ts`). The two
 * key-code guards read it the same way (`noKeyCodeText.ts` on each side). One
 * copy is what keeps an app's notes field and the host's composer on the same
 * rule.
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

/** Whether a codepoint names a key rather than text.
 *
 *  - **C0 control codes and DEL**, except tab and the two line breaks. A macOS
 *    arrow key carries 0x1C to 0x1F, left, right, up, down.
 *  - **AppKit's function-key constants**, 0xF700 to Mode Switch at 0xF747.
 *    Apple reserves the block to 0xF8FF but assigns nothing above 0xF747, and
 *    custom fonts put glyphs in the rest, so it stays typeable. */
function isKeyCode(code: number): boolean {
  if (code === 0x09 || code === 0x0a || code === 0x0d) return false;
  if (code < 0x20 || code === 0x7f) return true;
  return code >= 0xf700 && code <= 0xf747;
}

/** Whether a `beforeinput` inserts nothing but key codes, so it must be
 *  cancelled.
 *
 *  The desktop app's web view types one when an arrow key has nowhere to move
 *  the caret, and the field shows it as a square. `docs/temporary-measures.md`
 *  holds the mechanism and the removal condition.
 *
 *  Every character has to be one. A paste carries its content on
 *  `dataTransfer` and leaves `data` null, so pasted text is never refused. */
export function isKeyCodeTextInsertion(inputType: string, data: string | null): boolean {
  if (!inputType.startsWith('insert') || !data) return false;
  for (const char of data) {
    if (!isKeyCode(char.codePointAt(0) ?? 0)) return false;
  }
  return true;
}
