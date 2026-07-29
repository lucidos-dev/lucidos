/** Per-thread activity signal — fires for any SSE / replay event that
 *  appends to a thread's `events` Map or `streamingBuffer`, including
 *  pure-token streaming events that do not change `thread.meta` shape.
 *
 *  Purpose. `threadMap` itself is reserved for **meta-shape** changes
 *  (title, status, section, codingAgent* flags, child counts, etc.) so
 *  the many components that subscribe to it for meta — `attentionThreadCount`,
 *  `ThreadDrawer.ThreadList`, `PromptInput`, every visible `ChatExchange` —
 *  stop re-rendering on every token of a streaming CC subprocess. Components
 *  that need live data for the focused thread subscribe to the bump signal
 *  for THAT thread; an unrelated thread's stream doesn't fan out to them.
 *
 *  Implementation: a lazy-created `Signal<number>` per thread id. Subscribers
 *  via `getThreadEventsBump(id)` subscribe to ONLY that thread's signal — a
 *  bump on a different thread does not wake them. That's stricter than the
 *  `composeDrafts` / `draftPresentThreadIds` pattern (which uses one wide
 *  signal) because streaming-event rates are much higher than per-keystroke
 *  rates: a wide `Map` signal would fire every consumer on every stream
 *  token, defeating the gate this module exists to enforce.
 */

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

/** Run the flush. Exported for tests that bypass RAF (e.g. running under
 *  jsdom). Never call from production code — let `bumpThreadEvents` schedule. */
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

/** Mark `threadId` as having new event/streaming activity. Multiple bumps for
 *  the same thread within a frame collapse to one increment, and bumps across
 *  many threads share one RAF callback. Each bump writes to ONLY its thread's
 *  signal — subscribers to a different thread are not woken. */
export function bumpThreadEvents(threadId: string): void {
  pending.add(threadId);
  if (rafId !== null) return;
  if (typeof requestAnimationFrame === 'function') {
    rafId = requestAnimationFrame(() => {
      rafId = null;
      flushThreadEventsBumpsNow();
    });
  } else {
    // jsdom / node test path — flush synchronously so tests can observe the
    // increment without polyfilling RAF.
    flushThreadEventsBumpsNow();
  }
}

/** Read the current bump counter for a thread. Reading via this getter
 *  subscribes the caller to ONLY this thread's per-id signal — bumps on
 *  other threads do not re-fire the caller. */
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
