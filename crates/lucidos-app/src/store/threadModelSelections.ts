/** The Lucidos Agent model, reasoning effort and backend of an ACTIVE thread.
 *
 *  A thread remembers the pair it runs instead of snapping back to the account
 *  default on every new message. The engine holds that memory as thread events:
 *  the model, effort and provider stamped on each starter (`MessageReceived`,
 *  `TriggerStarted`), and the user's own pick between turns
 *  (`ChatSettingsChanged`, recorded through `POST /api/v1/chat/settings`).
 *  Resolution is `the thread's newest such event ?? the account default`
 *  (`currentModel` and `accountChatEffort`, backed by the `chat_model` /
 *  `chat_reasoning_efforts` preferences, editable in Settings). A tier and a
 *  backend are remembered with their model, so switching model carries neither.
 *
 *  Nothing here is held only in this tab: a pick on another device arrives as
 *  an event over SSE, and the face follows it. An active-thread pick never
 *  writes the account preference, which stays a Settings-only default. The
 *  engine resolves a follow-up's pair the same way
 *  (`PreferenceStore::last_thread_chat_settings`). */

import { threadMap, currentModel } from './store';
import { accountChatEffort, storedChatModel } from './actions/preferences';
import type { StoredEvent } from './thread-events';

type PairEvent = Extract<StoredEvent, { type: 'MessageReceived' | 'TriggerStarted' | 'ChatSettingsChanged' }>;

function isPairEvent(event: StoredEvent): event is PairEvent {
  return event.type === 'MessageReceived'
    || event.type === 'TriggerStarted'
    || event.type === 'ChatSettingsChanged';
}

/** The non-empty string `read` picks from the thread's most recent event that
 *  names a pair and carries one. Mirrors the backend's newest-by-sequence
 *  lookup, so the menu displays what the next turn will actually use.
 *  Synthetic failed-send rows carry no model, so the filter skips them.
 *
 *  Both starter kinds count, as does a pick: a chat turn starts with
 *  `MessageReceived` and a trigger fire with `TriggerStarted`, which emits no
 *  `MessageReceived` at all. */
function newestPairValue(
  threadId: string | null | undefined,
  read: (event: PairEvent) => unknown,
): string | undefined {
  if (!threadId) return undefined;
  const thread = threadMap.value.get(threadId);
  if (!thread) return undefined;
  let bestSeq = -Infinity;
  let bestValue: string | undefined;
  for (const [seq, event] of thread.events) {
    if (!isPairEvent(event)) continue;
    const value = read(event);
    if (typeof value !== 'string' || value === '') continue;
    if (seq > bestSeq) {
      bestSeq = seq;
      bestValue = value;
    }
  }
  return bestValue;
}

export function lastThreadModel(threadId: string | null | undefined): string | undefined {
  return newestPairValue(threadId, (event) => event.model);
}

/** The tier this thread last ran `model` at. Mirrors the backend's
 *  `last_thread_chat_settings`: a tier is remembered WITH its model, and
 *  `undefined` matches starters that stamped no model. */
function lastThreadReasoningEffort(
  threadId: string | null | undefined,
  model: string | undefined,
): string | undefined {
  return newestPairValue(threadId, (event) => ((event.model || undefined) === model ? event.reasoning_effort : undefined));
}

/** The backend this thread last pinned for `model`. Mirrors the backend's
 *  `last_thread_chat_settings`: a pick is remembered WITH its model, so a
 *  thread switched to another model does not carry the old backend along. */
export function lastThreadProvider(
  threadId: string | null | undefined,
  model: string,
): string | undefined {
  return newestPairValue(threadId, (event) => (event.model === model ? event.provider : undefined));
}

// --- Resolvers: the thread's events ?? account default ---

export function resolveActiveThreadModel(threadId: string | null | undefined): string {
  return lastThreadModel(threadId) ?? currentModel.value;
}

/** The backend the next turn on this thread pins, or `null` for the model's
 *  own default. */
export function resolveActiveThreadProvider(
  threadId: string | null | undefined,
  model: string,
): string | null {
  return lastThreadProvider(threadId, model) ?? null;
}

/** The tier the next turn on this thread runs at, for `model`: the thread's
 *  memory for `model`, then the account's tier for it. `null` sends no effort. */
export function resolveActiveThreadReasoningEffort(
  threadId: string | null | undefined,
  model: string,
): string | null {
  // The model the thread names, which is unset when it never stamped one.
  const named = model === resolveActiveThreadModel(threadId) ? lastThreadModel(threadId) : model;
  // A thread that named no model ran on the router's, so its tier never
  // carries onto a `chat_model` set since.
  const remembered = named === undefined && storedChatModel() !== null
    ? undefined
    : lastThreadReasoningEffort(threadId, named);
  return remembered ?? accountChatEffort(model);
}
