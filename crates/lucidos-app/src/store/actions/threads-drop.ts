
import { focusedThreadId, threadMap } from '../store';
import { forgetComposeState } from './compose';
import { forgetUnsentMessagesOfThread } from '../unsentMessages';
import { unfocusThread, visibleCandidatesAround } from './threads';
import { openListedThread } from './event-navigation';
import { forgetThreadEventsFailures, noteThreadDeleted } from './thread-loading';
import { removeThreadNavEntries } from './thread-navigation';
import { pruneRecents } from './searchRecents';

export function dropDeletedThreads(ids: readonly string[]): void {
  const gone = new Set(ids);
  if (gone.size === 0) return;

  const focused = focusedThreadId.value;
  const candidates = focused && gone.has(focused) ? visibleCandidatesAround(focused) : [];

  const next = new Map(threadMap.value);
  for (const id of gone) {
    next.delete(id);
    noteThreadDeleted(id);
    forgetComposeState(id);
    forgetUnsentMessagesOfThread(id);
    forgetThreadEventsFailures(id);
    removeThreadNavEntries(id);
    pruneRecents(id, 'threads');
  }
  threadMap.value = next;

  if (focused && gone.has(focused)) {
    const nextId = candidates.find((id) => !gone.has(id)) ?? null;
    if (nextId) openListedThread(nextId, { revealPane: false });
    else unfocusThread({ revealPane: false });
  }
}
