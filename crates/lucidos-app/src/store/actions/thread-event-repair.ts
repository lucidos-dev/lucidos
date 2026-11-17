import { threadMap } from '../store';
import { loadThreadEvents } from './thread-loading';

/** Rebuild the thread's events Map and force a full re-fetch. Called by the
 *  ThreadView watchdog when has()/get() on the long-lived Map return wrong
 *  results: iOS Safari can corrupt Map internals under memory pressure.
 *  Caller is responsible for the eligibility check (has CONTENT events but
 *  exchanges are empty) and retry capping. */
export function rebuildCorruptedThreadEvents(threadId: string): void {
  const thread = threadMap.value.get(threadId);
  if (!thread) return;
  thread.events = new Map(thread.events);
  thread.eventsLoaded = false;
  thread.lastDbSeq = 0;
  void loadThreadEvents(threadId);
}

