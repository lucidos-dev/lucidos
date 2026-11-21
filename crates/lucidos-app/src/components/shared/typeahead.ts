/** A key that types into a filter box: one printable char, no modifiers, and
 *  not Space, which a menu owes to its own scrolling and opening.
 *
 *  Shared by `Dropdown` and `ModelSelectionPicker`, so both search on the same
 *  keys. Pure, for testing. */
export function isTypeaheadKey(
  e: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey'>,
): boolean {
  return e.key.length === 1 && e.key !== ' '
    && !e.metaKey && !e.ctrlKey && !e.altKey;
}

/** Whether a keydown should START type-to-search: reveal the filter box and
 *  seed it, rather than navigate or select.
 *
 *  A menu opens without a filter box. An empty box with a caret in it is noise
 *  on a list the user opened to click a row. Once searching, the focused input
 *  owns every later keystroke, so this returns false and the key flows into it
 *  natively.
 *
 *  `freeText` is for a trigger that is ITSELF a text input: it is already the
 *  place typing goes, so nothing is revealed. */
export function isTypeaheadSeedKey(
  e: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey'>,
  opts: { searching: boolean; freeText?: boolean },
): boolean {
  return !opts.freeText && !opts.searching && isTypeaheadKey(e);
}
