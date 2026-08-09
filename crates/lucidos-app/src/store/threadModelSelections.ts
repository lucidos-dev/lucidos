/** Per-thread Lucidos Agent model + reasoning-effort memory for ACTIVE threads.
 *
 *  A thread remembers the model/effort it last ran with instead of snapping back
 *  to the account default on every new message. The DURABLE record of that is the
 *  model/reasoning_effort the backend stamps on each `MessageReceived` event — so
 *  resolution is `this thread's pending pick ?? the thread's last message ?? the
 *  account default` (`currentModel` / `reasoningEffort`, backed by the
 *  `chat_model` / `chat_reasoning_effort` preferences, editable in Settings).
 *
 *  This map holds ONLY a not-yet-sent pick made from the in-thread control menu.
 *  It is IN-MEMORY / ephemeral (unlike `composeSelections`, which is DB-backed):
 *  once the pick is sent it is stamped on the message and becomes the thread's
 *  remembered value, so `sendMessage` clears the entry after a successful send and
 *  a reload self-heals from the thread's events. Keeping it keyed per thread means
 *  a pick on thread A can never leak into thread B — and, per the "this thread
 *  only" decision, an active-thread pick NEVER writes the account preference
 *  (that stays a Settings-only default). The backend owns the follow-up fallback
 *  (see `PreferenceStore::resolve_chat_overrides_for_thread`); the frontend sends
 *  an override only when the user made an explicit pick this turn. */

import { signal } from '@preact/signals';
import { threadMap, currentModel, reasoningEffort } from './store';

export interface ThreadModelOverride {
  /** Lucidos Agent model id (the in-thread mirror of `chat_model`). */
  model?: string;
  /** Lucidos Agent reasoning effort. */
  reasoningEffort?: string;
}

/** Pending, not-yet-sent active-thread picks, keyed by thread id. */
export const threadModelSelections = signal<Map<string, ThreadModelOverride>>(new Map());

/** Stable empty override so read-then-fallback callers can't accidentally write
 *  a shared default. */
const EMPTY_OVERRIDE: ThreadModelOverride = Object.freeze({});

export function getThreadModelOverride(threadId: string | null | undefined): ThreadModelOverride {
  if (!threadId) return EMPTY_OVERRIDE;
  return threadModelSelections.value.get(threadId) ?? EMPTY_OVERRIDE;
}

/** Patch a thread's pending override. Callers pass only the fields they set.
 *  Returns a new Map/object so signal subscribers re-render. */
export function patchThreadModelOverride(threadId: string, patch: ThreadModelOverride): void {
  const prev = threadModelSelections.value.get(threadId) ?? EMPTY_OVERRIDE;
  const next: ThreadModelOverride = { ...prev, ...patch };
  const map = new Map(threadModelSelections.value);
  map.set(threadId, next);
  threadModelSelections.value = map;
}

/** Drop a thread's pending override (called by `sendMessage` after a successful
 *  send — the sent message now carries the value, so resolution falls to the
 *  thread's last message). No-op when there's nothing to clear. */
export function clearThreadModelOverride(threadId: string | null | undefined): void {
  if (!threadId || !threadModelSelections.value.has(threadId)) return;
  const map = new Map(threadModelSelections.value);
  map.delete(threadId);
  threadModelSelections.value = map;
}

/** The `field` value from the thread's most recent starter event that carried
 *  it. Mirrors the backend's newest-by-sequence lookup so the menu displays what
 *  the next send will actually use. Synthetic failed-send rows carry no model, so
 *  the field filter skips them.
 *
 *  Both starter kinds count, mirroring the backend's
 *  `IN ('MessageReceived', 'TriggerStarted')`: a chat turn starts with
 *  `MessageReceived`, while a trigger fire starts with `TriggerStarted` and
 *  emits no `MessageReceived` at all. Reading only the former would show the
 *  account model on a trigger thread however the trigger was pinned. */
function lastThreadMessageField(
  threadId: string | null | undefined,
  field: 'model' | 'reasoning_effort',
): string | undefined {
  if (!threadId) return undefined;
  const thread = threadMap.value.get(threadId);
  if (!thread) return undefined;
  let bestSeq = -Infinity;
  let bestValue: string | undefined;
  for (const [seq, event] of thread.events) {
    if (event.type !== 'MessageReceived' && event.type !== 'TriggerStarted') continue;
    const value = event[field];
    if (typeof value !== 'string' || value === '') continue;
    if (seq > bestSeq) {
      bestSeq = seq;
      bestValue = value;
    }
  }
  return bestValue;
}

export function lastThreadModel(threadId: string | null | undefined): string | undefined {
  return lastThreadMessageField(threadId, 'model');
}

export function lastThreadReasoningEffort(threadId: string | null | undefined): string | undefined {
  return lastThreadMessageField(threadId, 'reasoning_effort');
}

// --- Resolvers: pending pick ?? thread's last message ?? account default ---

export function resolveActiveThreadModel(threadId: string | null | undefined): string {
  return getThreadModelOverride(threadId).model ?? lastThreadModel(threadId) ?? currentModel.value;
}

export function resolveActiveThreadReasoningEffort(threadId: string | null | undefined): string {
  return (
    getThreadModelOverride(threadId).reasoningEffort ??
    lastThreadReasoningEffort(threadId) ??
    reasoningEffort.value
  );
}

export function _resetThreadModelSelectionsForTesting(): void {
  threadModelSelections.value = new Map();
}
