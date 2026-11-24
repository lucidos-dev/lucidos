
import { signal, type Signal } from '@preact/signals';

const perThread = new Map<string, Signal<number>>();

function getOrCreateSignal(threadId: string): Signal<number> {
  let s = perThread.get(threadId);
  if (!s) {
    s = signal(0);
    perThread.set(threadId, s);
  }
  return s;
}

const pending = new Set<string>();
let rafId: number | null = null;

export function flushThreadEventsBumpsNow(): void {
  if (rafId !== null && typeof cancelAnimationFrame === 'function') {
    cancelAnimationFrame(rafId);
  }
  rafId = null;
  if (pending.size === 0) return;
  for (const id of pending) {
    const s = getOrCreateSignal(id);
    s.value = s.value + 1;
  }
  pending.clear();
}

export function bumpThreadEvents(threadId: string): void {
  pending.add(threadId);
  if (rafId !== null) return;
  if (typeof requestAnimationFrame === 'function') {
    rafId = requestAnimationFrame(() => {
      rafId = null;
      flushThreadEventsBumpsNow();
    });
  } else {
    flushThreadEventsBumpsNow();
  }
}

export function getThreadEventsBump(threadId: string): number {
  return getOrCreateSignal(threadId).value;
}

export function _resetThreadEventsBumpForTesting(): void {
  pending.clear();
  if (rafId !== null && typeof cancelAnimationFrame === 'function') {
    cancelAnimationFrame(rafId);
  }
  rafId = null;
  perThread.clear();
}
