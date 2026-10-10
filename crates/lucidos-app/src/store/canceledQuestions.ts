/** What a thread's Cancel targeted, kept until the composer releases it.
 *
 *  The composer reads it and the chat actions write it. It lives in the store
 *  so the actions do not pull the composer's helpers into the entry chunk
 *  (ADR 0288). */

import { signal } from '@preact/signals';

/** For a thread whose Cancel was clicked while a question was on screen, the
 *  `tool_use_id` of the question that was pending at click time.
 *
 *  The cleanup effect (PromptInput) keys the optimistic `cancelingThreadIds`
 *  release off this. The targeted question stops being the latest pending one
 *  once it resolves as Canceled and the agent idles or re-asks. The flag then
 *  drops, so the morph button stops sticking in a disabled "Cancel...".
 *
 *  Without it, a cancel the agent answers by re-asking leaves the thread
 *  mid-turn forever (waiting → running → waiting), and the not-mid-turn
 *  release never fires. A running-turn cancel records no entry, having no
 *  question to key on, and falls back to the not-mid-turn release. */
export const canceledQuestionByThread = signal<Map<string, string>>(new Map());

/** Record (or clear) the question a thread's Cancel targeted. Pass `undefined`
 *  for a running-turn cancel so any stale entry is dropped rather than
 *  mis-keying the next release. */
export function setCanceledQuestion(threadId: string, toolUseId: string | undefined): void {
  const map = canceledQuestionByThread.value;
  if (toolUseId === undefined && !map.has(threadId)) return;
  const next = new Map(map);
  if (toolUseId === undefined) next.delete(threadId);
  else next.set(threadId, toolUseId);
  canceledQuestionByThread.value = next;
}

/** Threads whose Cancel was clicked while the thread was already
 *  `waiting_for_user_answer`, with a question OR permission card on screen.
 *
 *  The cleanup effect reads this to keep a card cancel bridged through
 *  `waiting_for_user_answer`, via `shouldClearCanceling`'s awaiting branch. A
 *  generic running-turn cancel records no entry here. It is released the
 *  instant the turn leaves `running`, so a superseded cancel that lands on a
 *  new card cannot wedge "Canceling" forever.
 *
 *  Complements `canceledQuestionByThread`, which covers `UserQuestionAsked`
 *  cards only. Permission cards set this one but not that one. */
export const canceledWhileAwaitingByThread = signal<Set<string>>(new Set());

/** Record (or clear) whether a thread's Cancel was clicked while awaiting a
 *  user answer. Clear it (pass `false`) on the same release the optimistic
 *  canceling flag drops, so a later running-turn cancel isn't mis-keyed. */
export function setCanceledWhileAwaiting(threadId: string, awaiting: boolean): void {
  const set = canceledWhileAwaitingByThread.value;
  if (awaiting === set.has(threadId)) return;
  const next = new Set(set);
  if (awaiting) next.add(threadId);
  else next.delete(threadId);
  canceledWhileAwaitingByThread.value = next;
}
