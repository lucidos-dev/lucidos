import { computed } from '@preact/signals';
import { threadMap } from '../store';
import type { ThreadState } from '../thread-events';
import { focusThread } from './threads';

/** The workspace's home thread (ADR 0362, ADR 0411). Every workspace has one.
 *  No thread list draws it: the desktop thread header and the mobile Lucidos
 *  menu each carry a Home entry instead. */
export function findHomeThread(threads: Iterable<ThreadState>): ThreadState | undefined {
  for (const t of threads) if (t.meta.home) return t;
  return undefined;
}

/** The home thread's id, or null until the thread list has it. A computed id,
 *  so a Home entry re-renders when Home arrives, not on every thread update. */
export const homeThreadId = computed<string | null>(
  () => findHomeThread(threadMap.value.values())?.meta.id ?? null,
);

/** Open the home thread. A no-op while there is none to open, which the
 *  Home entries already rule out by not rendering. */
export function openHomeThread(): void {
  const id = homeThreadId.value;
  if (id) focusThread(id);
}
