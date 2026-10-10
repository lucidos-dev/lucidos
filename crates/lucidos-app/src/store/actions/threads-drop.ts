/** Dropping a deleted thread family from the client.
 *
 *  Apart from `threads-delete.ts` because the event stream calls it on every
 *  device, while the dialog only ever runs on one. Together, the stream pulled
 *  the dialog and the thread actions into the entry chunk (ADR 0288). */

import { focusedThreadId, threadMap } from '../store';
import { forgetComposeState } from './compose';
import { forgetUnsentMessagesOfThread } from '../unsentMessages';
import { focusThread, unfocusThread, visibleCandidatesAround } from './threads';
import { forgetThreadEventsFailures } from './thread-loading';
import { removeThreadNavEntries } from './thread-navigation';
import { pruneRecents } from './entityReferences';

/** Drop a deleted family from the client.
 *
 *  Idempotent, and called from two places: the delete this device made, and
 *  the `ThreadsDeleted` frame every other device receives. Neither can wait for
 *  the other, and running both is harmless.
 *
 *  The focus hand-off does NOT reveal the thread pane, because deleting is not
 *  navigation. The thread drawer is its own pane on mobile, so a reveal would
 *  swipe the user away on every tap.
 */
export function dropDeletedThreads(ids: readonly string[]): void {
  const gone = new Set(ids);
  if (gone.size === 0) return;

  const focused = focusedThreadId.value;
  // Snapshot the position anchor BEFORE the rows leave the map: once they are
  // gone, `visibleCandidatesAround` cannot find the one the user was on.
  const candidates = focused && gone.has(focused) ? visibleCandidatesAround(focused) : [];

  const next = new Map(threadMap.value);
  for (const id of gone) {
    next.delete(id);
    // A thread that will never be fetched again owes the same cleanup a
    // discarded draft does: nothing may hold a pointer into it. `compose.ts`
    // owns eight private per-thread maps, so it does its own half.
    forgetComposeState(id);
    forgetUnsentMessagesOfThread(id);
    forgetThreadEventsFailures(id);
    removeThreadNavEntries(id);
    pruneRecents(id, 'threads');
  }
  threadMap.value = next;

  if (focused && gone.has(focused)) {
    const nextId = candidates.find((id) => !gone.has(id)) ?? null;
    if (nextId) focusThread(nextId, { revealPane: false });
    else unfocusThread({ revealPane: false });
  }
}
