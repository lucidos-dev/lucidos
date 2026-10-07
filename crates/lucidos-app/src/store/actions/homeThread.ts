import { computed } from '@preact/signals';
import { threadMap } from '../store';
import type { ThreadState } from '../thread-events';
import { homeThreadEnabled } from './preferences';
import { focusThread } from './threads';

/** The workspace's home thread, or none while its switch is off (ADR 0362).
 *  No thread list draws it: the desktop thread header and the mobile Lucidos
 *  menu each carry a Home entry instead. */
export function findHomeThread(
  threads: Iterable<ThreadState>,
  homeOn: boolean,
): ThreadState | undefined {
  if (!homeOn) return undefined;
  for (const t of threads) if (t.meta.home) return t;
  return undefined;
}

/** The home thread's id, or null while there is none to open. A computed id,
 *  so a Home entry re-renders when Home comes or goes, not on every thread
 *  update. */
export const homeThreadId = computed<string | null>(
  () => findHomeThread(threadMap.value.values(), homeThreadEnabled())?.meta.id ?? null,
);

/** Open the home thread. A no-op while there is none to open, which the
 *  Home entries already rule out by not rendering. */
export function openHomeThread(): void {
  const id = homeThreadId.value;
  if (id) focusThread(id);
}
