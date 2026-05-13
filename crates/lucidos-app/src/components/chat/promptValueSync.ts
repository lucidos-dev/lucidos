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

/** Skip the textarea sync only when THIS specific element is the active one
 *  and already holds locally-typed content. Element-identity (not "any
 *  prompt-input with this thread id is active") matters: SplitLayout and
 *  MobileSwipeContainer mount PromptInput twice with the same
 *  `data-thread-id`, and the unfocused copy must still re-sync after a
 *  Send on the focused copy. Empty `el.value` always re-syncs so the
 *  persisted draft reaches the textarea on initial autofocus. */
export function shouldSkipSyncWhileEditing(
  el: HTMLTextAreaElement,
  sameThread: boolean,
  thisElementActive: boolean,
): boolean {
  return sameThread && thisElementActive && el.value.length > 0;
}
