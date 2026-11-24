import { computed } from '@preact/signals';
import { threadMap } from '../store';
import type { ThreadState } from '../thread-events';
import { focusThread } from './threads';

export function findHomeThread(threads: Iterable<ThreadState>): ThreadState | undefined {
  for (const t of threads) if (t.meta.home) return t;
  return undefined;
}

export const homeThreadId = computed<string | null>(
  () => findHomeThread(threadMap.value.values())?.meta.id ?? null,
);

export function isHomeThread(threadId: string): boolean {
  return threadId === homeThreadId.value;
}

export function openHomeThread(): void {
  const id = homeThreadId.value;
  if (id) focusThread(id);
}
