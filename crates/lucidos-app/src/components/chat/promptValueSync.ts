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

/** What the pending override did to the draft.
 *
 *  `replace` throws the old text away, so the caret offset indexes characters
 *  that are gone. Restoring it drops the user mid-sentence in copy they did not
 *  write. `append` leaves the prefix intact, so the offset still means what it
 *  did and the caret stays where they left it. */
export type PromptOverrideKind = 'replace' | 'append';

/** Whether the pending override replaced the whole draft. Written just before
 *  the counter and read beside it, so one render sees both. Only ever read on
 *  the render a bump produces, which is why a stale value between bumps cannot
 *  matter. */
export const promptOverrideReplacesDraft = signal(false);

/** Request a one-shot forced textarea sync — call AFTER writing the draft
 *  (`updateCompose`) so the effect observes both the new text and the bumped
 *  counter in one render. See {@link promptOverrideSyncSeq}. `kind` is required
 *  rather than defaulted: it decides where the caret lands, and a caller that
 *  did not think about it is the bug this argument exists to prevent. */
export function requestPromptOverrideSync(kind: PromptOverrideKind): void {
  promptOverrideReplacesDraft.value = kind === 'replace';
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
 *  and already holds locally-typed content.
 *
 *  Element-identity matters, rather than "any prompt-input with this thread id
 *  is active". Only the focused textarea can hold an in-flight keystroke, so a
 *  copy that is not focused must still re-sync after a Send. Empty `el.value`
 *  always re-syncs, so the persisted draft reaches the textarea on initial
 *  autofocus. */
export function shouldSkipSyncWhileEditing(
  el: HTMLTextAreaElement,
  sameThread: boolean,
  thisElementActive: boolean,
): boolean {
  return sameThread && thisElementActive && el.value.length > 0;
}

/** What an EMPTY draft is allowed to do to the textarea it is being synced into.
 *
 *  `clear` takes the empty draft, which is every ordinary case and what the
 *  sync has always done. `adopt` leaves the box alone, because it holds
 *  characters the store never saw, and hands them to the draft instead.
 *
 *  **The composer keeps the message twice, and the two copies disagree here.**
 *  `resolveComposerText` rules on exactly this state at SEND time and keeps
 *  what is on screen. This is the same verdict at SYNC time. The opposite one
 *  used to stand here: a clear landing under the user's fingers then erased
 *  what they had typed, in silence. Ten paths clear a draft asynchronously and
 *  each carries its own guard, so one guard being wrong anywhere was enough.
 *  The plan is
 *  `docs/plans/2026-08-29-the-composer-never-erases-what-you-typed.md`. */
export type EmptyDraftSync = 'clear' | 'adopt';

/** `typedSinceComposerWrote` is the discriminator, NOT focus. A box holding
 *  text a previous sync put there carries a synced draft. A peer's clear must
 *  still empty that, or it ghosts (`applyRemoteCompose`'s rule), so only
 *  characters the user entered are protected.
 *
 *  Whitespace alone is not content by `composeHasContent`, so adopting it would
 *  write an empty draft. On a composing thread `updateCompose` reads that as
 *  the draft being emptied and auto-discards, clearing the draft and re-entering
 *  this branch. Clearing is the answer that terminates. */
export function resolveEmptyDraftSync(args: {
  domText: string;
  typedSinceComposerWrote: boolean;
  thisElementActive: boolean;
  sameThread: boolean;
}): EmptyDraftSync {
  if (!args.sameThread || !args.thisElementActive) return 'clear';
  if (!args.typedSinceComposerWrote) return 'clear';
  return args.domText.trim().length > 0 ? 'adopt' : 'clear';
}
