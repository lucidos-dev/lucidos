import { signal } from '@preact/signals';

/** One-shot "force the next textarea sync" ticket. A deliberate programmatic
 *  compose override — a welcome starter suggestion replacing an in-progress
 *  draft — must reach the textarea even while it is focused + non-empty, which
 *  `shouldSkipSyncWhileEditing` otherwise blocks to protect in-flight typing.
 *  Without the bypass the draft signal (and the drawer row) update but the
 *  visible prompt stays stale — the "not reflected in the prompt text" bug.
 *  Bumping this counter makes PromptInput's sync effect force the very next sync
 *  it observes. It is a plain monotonic counter (not the text/thread) so a repeat
 *  override of the same text still fires. */
export const promptOverrideSyncSeq = signal(0);

/** Request a one-shot forced textarea sync — call AFTER writing the draft
 *  (`updateCompose`) so the effect observes both the new text and the bumped
 *  counter in one render. See {@link promptOverrideSyncSeq}. */
export function requestPromptOverrideSync(): void {
  promptOverrideSyncSeq.value += 1;
}

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
