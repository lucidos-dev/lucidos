/** Sync a textarea's value to `text`. Returns true when the DOM changed.
 *
 *  Browsers snap selectionStart/End to the end of the new value on every
 *  `el.value =` reassignment. With `preserveCursor` we read selection before
 *  the write and restore it (clamped to text.length) after. Pass `false` on
 *  thread switch — the previous offset is meaningless on the new text. */
export function syncTextareaValue(
  el: HTMLTextAreaElement,
  text: string,
  preserveCursor: boolean,
): boolean {
  if (el.value === text) return false;
  const start = preserveCursor ? el.selectionStart : null;
  const end = preserveCursor ? el.selectionEnd : null;
  el.value = text;
  if (start !== null && end !== null) {
    el.setSelectionRange(
      Math.min(start, text.length),
      Math.min(end, text.length),
    );
  }
  return true;
}

/** Decide whether to skip the textarea sync to protect an in-flight
 *  keystroke. Skip ONLY when the user is focused here AND has typed
 *  something locally (`el.value` non-empty). On initial autofocus after
 *  page reload el.value is empty, and the persisted draft text MUST reach
 *  the textarea — otherwise the input shows blank while the store and
 *  drawer label still reflect the saved draft. */
export function shouldSkipSyncWhileEditing(
  el: HTMLTextAreaElement,
  sameThread: boolean,
  focusedHere: boolean,
): boolean {
  return sameThread && focusedHere && el.value.length > 0;
}
