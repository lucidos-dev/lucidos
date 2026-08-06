import {
  threadsLoaded,
  panelUrl,
  panelTitle,
  showToast,
  focusedThreadId,
  threadMap,
  selectedScope,
  selectedCodingAgent,
  scopeToFolder,
  codingAgentPendingModel,
  codingAgentPendingReasoningEffort,
  cancelingThreadIds,
  removingQueuedMessageIds,
  queuedMessageRemovalKey,
  setFocusedThread,
  effectiveThreadStatus,
} from '../store';
import type { ChatContext } from './chatContext';
import type { ChatRequestBody } from '../../api/types';
import { submitChat, cancelChat, stopClaudeCode, isTransportError, removeQueuedMessage as removeQueuedMessageRequest, ApiError, type CodingAgentModelValue, type CodingAgentReasoningEffort } from '../../api/client';
import { getUnreachableEngineMsg } from './connection';
import { getDeviceId, pendingDeviceRegistration } from './devices';
import { generateUuid } from '../../utils/uuid';
import { handleEvent, makeOptimisticThreadState, computeExchanges, queuedMessagesFromExchanges, type StoredEvent, type QueuedMessage } from '../thread-events';
import { getDraft } from '../composeDrafts';
import { updateCompose } from './compose';
import { requestPromptOverrideSync } from '../../components/chat/promptValueSync';
import { bumpThreadEvents } from '../threadActivity';
import { getThreadModelOverride, clearThreadModelOverride } from '../threadModelSelections';
import { pushThreadNavState, removeThreadNavEntries } from './thread-navigation';
import { revealThreadPane } from './pane';
import { scrollToBottom } from '../../components/chat/scrollState';
import { setCanceledQuestion, setCanceledWhileAwaiting } from '../../components/chat/prompt-input-helpers';
import { refreshThreadEvents, forgetThreadEventsFailures } from './thread-loading';
import { markThreadRerenderStart } from '../../utils/threadOpenMarks';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { isTauri } from '../../utils/platform';
import { getWebviewContent } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';

/** Safety timeout (ms) for pending messages. If SSE doesn't deliver the
 *  MessageReceived event within this window, we force-refresh thread events
 *  and clear the stale pending message. Prevents "Requesting..." getting
 *  stuck indefinitely when SSE drops after submitChat() succeeds. */
export const PENDING_MESSAGE_SAFETY_MS = 30_000;

/** How long a send waits for the thread's previous send to settle before going
 *  out anyway. `mutatingFetch` deliberately has no client-side timeout (a chat
 *  POST is not idempotent, so it must never be retried behind the user's back),
 *  which means a POST stalled on a half-open mobile connection can stay pending
 *  forever. Without a ceiling here, that one hung request would silently
 *  swallow every later message on the thread, which is far worse than the
 *  reordering the chain exists to prevent. Sized above a slow-but-alive
 *  cellular round trip and below `PENDING_MESSAGE_SAFETY_MS`, so a released
 *  send still gets its own safety sweep. */
export const SEND_CHAIN_MAX_WAIT_MS = 15_000;

/** Per-thread tail of the send chain: a promise that settles when the thread's
 *  most recently issued `submitChat` settles. It NEVER rejects, so one failed
 *  send cannot poison the chain for the rest of the thread.
 *
 *  This is what keeps a device's messages in the order the user pressed send.
 *  `MessageReceived.created` is stamped when the engine emits the event, and
 *  the request carries no client-side ordering data, so two POSTs in flight at
 *  once can be delivered out of order and the reversal is then unrecoverable:
 *  the later message wins the race, starts the turn, and the earlier one is
 *  queued and injected into it as a follow-up. See
 *  `docs/plans/2026-07-30-serialize-chat-sends-per-thread.md`.
 *
 *  Keyed per thread: two different threads are independent conversations and
 *  must not queue behind each other. */
const sendChains = new Map<string, Promise<void>>();

/** Resolve when `p` settles, or after `ms`, whichever comes first. `p` never
 *  rejects (see `sendChains`), so the success arm alone covers both outcomes. */
function settledOrTimedOut(p: Promise<void>, ms: number): Promise<void> {
  return new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, ms);
    void p.then(() => {
      clearTimeout(timer);
      resolve();
    });
  });
}

/** This send's place in its thread's chain: what to await before POSTing, and
 *  the release that lets the next send go. */
interface SendSlot {
  /** `null` when nothing was ahead of this send. The caller must then skip the
   *  await entirely rather than await an already-resolved promise: awaiting one
   *  still defers a microtask, which would push the POST out of the caller's
   *  synchronous turn. `sendMessage` dispatches `submitChat` synchronously on
   *  the ordinary single-send path, and callers observe that (the compose
   *  suite asserts on the fetch mock right after calling `sendFollowup`,
   *  without awaiting). Serializing sends must not change when a lone send
   *  goes out. */
  waitForTurn: Promise<void> | null;
  release: () => void;
}

/** Claim the thread's next chain slot. **Synchronous, and that is the point:**
 *  the slot must be taken in the order `sendMessage` is CALLED, not in the
 *  order each call happens to reach its POST.
 *
 *  `sendMessage` can await before it gets there (`getWebviewContent()` on the
 *  Tauri panel path), and two of those awaits resolve in whatever order the
 *  webview answers. Claiming the slot at POST time would let the second send
 *  overtake the first there and hand the chain its slots reversed, reproducing
 *  the exact bug the chain exists to prevent, just one layer up. Claiming it
 *  here makes the guarantee independent of whatever awaits get added above. */
function enterSendChain(threadId: string): SendSlot {
  const predecessor = sendChains.get(threadId);
  let release!: () => void;
  const link = new Promise<void>((resolve) => {
    release = resolve;
  });
  sendChains.set(threadId, link);
  // Drop the entry once the chain drains, so the map doesn't grow one
  // permanent promise per thread the user has ever sent to. Guarded on
  // identity: a send that chained behind this one owns the slot now.
  void link.then(() => {
    if (sendChains.get(threadId) === link) sendChains.delete(threadId);
  });
  return {
    waitForTurn: predecessor ? settledOrTimedOut(predecessor, SEND_CHAIN_MAX_WAIT_MS) : null,
    release,
  };
}

type RemovedPendingMessage = {
  index: number;
  message: {
    text: string;
    eventId: string;
    created: string;
    image_hashes?: string[];
  };
};

/** Remove an optimistic pending message from a thread.
 *  Without cleanup, the thread stays stuck in "Requesting..." forever
 *  (pendingUserMessages never cleared → effectiveThreadStatus returns 'running'). */
export function removePendingMessage(threadId: string, eventId: string): RemovedPendingMessage | null {
  const t = threadMap.value.get(threadId);
  if (!t) return null;
  const idx = t.pendingUserMessages.findIndex(m => m.eventId === eventId);
  if (idx !== -1) {
    const [message] = t.pendingUserMessages.splice(idx, 1);
    threadMap.value = new Map(threadMap.value);
    // Same contract as addPendingMessage / the unreachable-engine path:
    // `activeExchanges` subscribes only to the per-thread bump and reads
    // threadMap via .peek(), so without this the stale 'Requesting...'
    // synthetic exchange (composed from pendingUserMessages) keeps painting
    // in the focused ThreadView until the next SSE event for this thread.
    bumpThreadEvents(threadId);
    return { index: idx, message };
  }
  return null;
}

function restorePendingMessage(threadId: string, removed: RemovedPendingMessage | null): void {
  if (!removed) return;
  const t = threadMap.value.get(threadId);
  if (!t || t.pendingUserMessages.some(m => m.eventId === removed.message.eventId)) return;
  const idx = Math.max(0, Math.min(removed.index, t.pendingUserMessages.length));
  t.pendingUserMessages.splice(idx, 0, removed.message);
  threadMap.value = new Map(threadMap.value);
  bumpThreadEvents(threadId);
}

/** Outcome of a single queued-message retract: `removed` (tombstone persisted),
 *  `already-injected` (the loop consumed it first — 409; it's now part of the
 *  running response), or `failed` (transport/other). */
export type QueuedRemovalOutcome = 'removed' | 'already-injected' | 'failed';

type QueuedRemovalResult = { outcome: QueuedRemovalOutcome; error?: unknown };

/** In-flight retract promises keyed by `queuedMessageRemovalKey`, so a second
 *  retract for the same message (trash-then-Stop, or a double click) AWAITS the
 *  first's real outcome instead of assuming success. Assuming success let Stop
 *  append the text to compose + cancel while the removal was still pending — if
 *  that removal then failed/409'd, the message re-ran or duplicated. */
const inFlightQueuedRemovals = new Map<string, Promise<QueuedRemovalResult>>();

/** Retract one queued message via the `QueuedMessageRemoved` tombstone. Shared
 *  by the per-message trash button (`removeQueuedMessage`) and the
 *  Stop-clears-queue path (`clearQueuedMessagesToCompose`). Optimistically drops
 *  the pending row and rolls it back on failure; returns the outcome (never
 *  throws) so each caller decides how to surface it. Deduped + awaited across
 *  concurrent callers so the outcome a caller sees is the ACTUAL request result. */
function retractQueuedMessage(threadId: string, messageId: string): Promise<QueuedRemovalResult> {
  const key = queuedMessageRemovalKey(threadId, messageId);
  const existing = inFlightQueuedRemovals.get(key);
  if (existing) return existing;

  const run = (async (): Promise<QueuedRemovalResult> => {
    removingQueuedMessageIds.value = new Set([...removingQueuedMessageIds.value, key]);
    const removedPending = removePendingMessage(threadId, messageId);
    try {
      await removeQueuedMessageRequest(threadId, messageId);
      return { outcome: 'removed' };
    } catch (err) {
      const next = new Set(removingQueuedMessageIds.value);
      next.delete(key);
      removingQueuedMessageIds.value = next;
      restorePendingMessage(threadId, removedPending);
      const alreadyInjected = err instanceof ApiError && err.httpCode === 409;
      return { outcome: alreadyInjected ? 'already-injected' : 'failed', error: err };
    } finally {
      inFlightQueuedRemovals.delete(key);
    }
  })();
  inFlightQueuedRemovals.set(key, run);
  return run;
}

export async function removeQueuedMessage(threadId: string, messageId: string): Promise<void> {
  const { outcome, error } = await retractQueuedMessage(threadId, messageId);
  if (outcome === 'removed') return;
  // Non-success (transport error OR a 409 race where the loop injected it just
  // now): re-sync so the row reflects truth and tell the user it didn't take.
  void refreshThreadEvents(threadId);
  showToast(`Failed to remove queued message: ${errorDetail(error)}`, 'error');
}

/** Mark one pending row as never-confirmed: the safety refetch gave up on it.
 *  The row stays so the user's text remains visible, but it stops counting as a
 *  turn in flight (`effectiveThreadStatus`). Still swapped for the real event by
 *  `handleEvent` if one ever arrives, since the match is on `eventId`. */
function markPendingUnconfirmed(threadId: string, eventId: string): void {
  const thread = threadMap.value.get(threadId);
  if (!thread) return;
  const pending = thread.pendingUserMessages.find(p => p.eventId === eventId);
  if (!pending || pending.unconfirmed) return;
  pending.unconfirmed = true;
  threadMap.value = new Map(threadMap.value);
  // Same per-thread bump pairing as removePendingMessage: the focused
  // ThreadView reads the row through the events bell, not the map write.
  bumpThreadEvents(threadId);
}

/** Remove pending messages older than PENDING_MESSAGE_SAFETY_MS.
 *  Called by the safety timer after submitChat() succeeds — if SSE dropped
 *  and refreshThreadEvents() didn't clear the pending message, this forcefully
 *  removes it so the thread stops showing "Requesting..." forever. */
export function clearStalePendingMessages(threadId: string): void {
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.pendingUserMessages.length === 0) return;

  const now = Date.now();
  const before = thread.pendingUserMessages.length;
  thread.pendingUserMessages = thread.pendingUserMessages.filter(
    p => now - new Date(p.created).getTime() < PENDING_MESSAGE_SAFETY_MS,
  );
  if (thread.pendingUserMessages.length < before) {
    threadMap.value = new Map(threadMap.value);
    // Same per-thread bump pairing as removePendingMessage — see comment
    // there. Without this the stale 'Requesting...' row that this safety
    // timer exists to clear keeps painting in the focused ThreadView.
    bumpThreadEvents(threadId);
  }
}

/** How long after PENDING_MESSAGE_SAFETY_MS to run a second refresh.
 *  If the CC process died and cleanup emitted ResponseAborted for lost
 *  follow-ups, this second refresh picks up those events so the exchange
 *  transitions from "Requesting..." to "Aborted". */
export const STALE_EXCHANGE_FOLLOWUP_MS = 60_000;

/** How many safety refetches may fail to land before the retry gives up (it
 *  keeps the pending row either way). Three, i.e. ~90s: long enough that a
 *  transient outage or a lagging SSE resolves first, which is the case the retry
 *  exists for, and bounded because a refetch can also decline permanently (the
 *  thread's events never loaded), which no amount of retrying changes and which
 *  would otherwise poll for the life of the page. */
const PENDING_CLEANUP_MAX_ATTEMPTS = 3;

/** Schedule a safety check that fires after PENDING_MESSAGE_SAFETY_MS.
 *  If the pending message is still present (SSE missed the MessageReceived),
 *  force-refresh thread events and clear any stale pending messages.
 *  A second refresh fires later to catch backend-emitted terminal events
 *  (e.g. ResponseAborted for lost follow-ups) that weren't ready at 30s.
 *
 *  The refetch IS the recovery. `schedulePendingCleanup` is only reached after
 *  `submitChat()` resolved, so the MessageReceived is already persisted in the
 *  DB — a successful refresh surfaces it and `handleEvent` swaps the optimistic
 *  row for it. The force-clear is the fallback for a GENUINE backend loss, but
 *  ONLY a refetch that actually SUCCEEDED can prove the event is absent. A
 *  refetch that FAILED (transient host contention / offline) proves nothing;
 *  clearing then would force-drop a message that is safely persisted — the
 *  user's just-sent message vanishes from the thread. That is the
 *  `coding-agent-follow-ups` "follow-up lost entirely under rapid send-while-
 *  working" flake: under load the MessageReceived SSE lags past 30s AND the
 *  safety refetch times out, so the unconditional clear destroyed a persisted
 *  follow-up. On a refetch that never landed we reschedule instead, up to
 *  `PENDING_CLEANUP_MAX_ATTEMPTS`, and then stop retrying while KEEPING the row:
 *  exhausting the retries proves no more than one failure did. The guard at the
 *  top also exits early once the pending is gone, so the retry usually ends the
 *  moment SSE catches up. */
export function schedulePendingCleanup(threadId: string, eventId: string, attempt = 1): void {
  setTimeout(async () => {
    const thread = threadMap.value.get(threadId);
    if (!thread || !thread.pendingUserMessages.some(p => p.eventId === eventId)) return;
    // `refreshThreadEvents` never rejects, so the `.catch` this used to gate on
    // could never fire and the force-drop below ran on every outcome, including
    // a refetch that got no answer. It reports whether a snapshot actually
    // LANDED instead, which is the only thing that can prove the event absent.
    const refetchOk = await refreshThreadEvents(threadId);
    if (refetchOk) {
      clearStalePendingMessages(threadId);
    } else if (attempt < PENDING_CLEANUP_MAX_ATTEMPTS) {
      // No answer, so nothing is proven. Don't drop a persisted message.
      schedulePendingCleanup(threadId, eventId, attempt + 1);
    } else {
      // Out of attempts. Stop POLLING, but keep the row: running out of tries
      // proves nothing either, and the two failure modes are not comparable. A
      // kept row is a visible, honest "this send was never confirmed" that
      // `handleEvent` swaps for the real event the moment one arrives, and that
      // a reload clears (it is in-memory only). Dropping it would silently
      // delete a message the user sent and the engine persisted, which is the
      // bug this whole gate exists for.
      //
      // Marked rather than merely kept, because a bare pending row makes
      // `effectiveThreadStatus` report 'running'. Left counted, the thread would
      // sit mid-turn for the life of the page: out of Review even once it
      // proposes a change, inside `runningThreadCount`, and showing a Stop with
      // nothing to stop.
      markPendingUnconfirmed(threadId, eventId);
      console.warn(`[Chat] pending message ${eventId} unconfirmed after ${attempt} refetches that never landed; keeping the row`);
    }
  }, PENDING_MESSAGE_SAFETY_MS);

  // CC threads only: pick up terminal events (ResponseAborted) emitted during
  // CC cleanup that weren't ready at the 30s mark.
  const existing = threadMap.value.get(threadId);
  if (existing?.meta.channel === 'claude_code') {
    setTimeout(async () => {
      const thread = threadMap.value.get(threadId);
      if (!thread || thread.meta.status !== 'running') return;
      await refreshThreadEvents(threadId);
    }, STALE_EXCHANGE_FOLLOWUP_MS);
  }
}

/** Add an optimistic pending message to a thread so the user sees it immediately. */
function addPendingMessage(
  threadId: string,
  message: string,
  eventId: string,
  imageHashes?: string[],
): void {
  const map = threadMap.value;
  const thread = map.get(threadId);
  if (thread) {
    thread.pendingUserMessages.push({
      text: message,
      eventId,
      created: new Date().toISOString(),
      image_hashes: imageHashes,
    });
    if (focusedThreadId.value === threadId) {
      scrollToBottom();
      // Perf: stamp the open→paint re-render span for the `thread-rerender` mark
      // (a follow-up send on the focused thread re-renders the whole exchange
      // list). ThreadView fires once on the next render. Focused-only — a
      // background thread's optimistic insert doesn't render. Fire-and-forget
      // telemetry; see utils/threadOpenMarks.ts + utils/renderPhaseTimers.ts.
      markThreadRerenderStart(threadId, { ...currentPerfBaseline(), cause: 'send' });
    }
    threadMap.value = new Map(map);
    // `computeExchanges` reads `thread.pendingUserMessages` to synthesize the
    // optimistic user-message row, but `activeExchanges` no longer subscribes
    // to `threadMap` — it subscribes to the per-thread events bump (see
    // `store/threadActivity.ts`). Without this bump the focused-thread
    // computeds (`activeExchanges` in CreateThreadView + ThreadPane,
    // `activeStreamingBuffer` in ThreadView) keep their cached value and the
    // synthetic exchange doesn't render until the next SSE event arrives.
    bumpThreadEvents(threadId);
  }
}

// `loadRepositories` lives in `./repositoriesLoader` so SSE-handler modules
// can refresh the repositories cache without dragging in chat.ts's transitive
// import tree. Re-exported here so existing call sites
// (`import { loadRepositories } from '../store/actions/chat'`) keep working.
export { loadRepositories } from './repositoriesLoader';

/**
 * Send a chat message. Side effects (modals, refreshes) are handled by
 * thread-sync.ts via ThreadEvent SSE events — no listener registry needed.
 */
export async function sendMessage(
  message: string,
  imageHashes?: string[],
  options?: {
    useCodingAgent?: boolean;
    context?: ChatContext | null;
    threadId?: string;
    focus?: boolean;
    // Per-draft selection carried from `sendCompose` (compose first-send). When
    // present these win over the global signals; when absent (raw-new sends +
    // follow-ups) the globals below are used, preserving the old behavior. The
    // Lucidos Agent path uses model/reasoningEffort; the coding-agent path uses
    // ccModel/ccReasoningEffort. `undefined` = not supplied (fall back);
    // `ccModel: null` = the explicit "default" pick (omit cc_model).
    modelOverride?: string;
    reasoningEffortOverride?: string;
    ccModelOverride?: CodingAgentModelValue | null;
    ccReasoningEffortOverride?: CodingAgentReasoningEffort | null;
  },
): Promise<void> {
  threadsLoaded.value = true;
  const eventId = generateUuid();
  const explicitThreadId = options?.threadId;
  const shouldFocus = options?.focus ?? true;
  const isNewThread = explicitThreadId === undefined && focusedThreadId.value === null;
  const threadId = explicitThreadId || focusedThreadId.value || eventId;
  // Claim the chain slot before ANY await below, so this send's place in the
  // thread's order is the order the user pressed send. Released in the
  // `finally` around the POST.
  const sendSlot = enterSendChain(threadId);

  if (shouldFocus) {
    setFocusedThread(threadId);
    if (isNewThread) {
      pushThreadNavState({ type: 'thread', id: threadId });
      // A brand-new thread spawned from another pane (e.g. the new-app form in
      // the content pane) must surface on the thread pane, mirroring focusThread.
      // Compose/follow-up sends pass an explicit threadId so isNewThread is
      // false — they're already on the thread pane, so this only fires for raw
      // new sends, where it's the correct landing.
      revealThreadPane();
    }
  }

  // Snapshot the thread BEFORE the optimistic insert below — the insert
  // creates a state='active' thread for raw new threads, which would
  // otherwise be indistinguishable from an active follow-up further down.
  const threadBeforeSend = threadMap.value.get(threadId);

  const map = threadMap.value;
  if (!map.has(threadId)) {
    map.set(threadId, makeOptimisticThreadState({
      id: threadId,
      title: message.slice(0, 40),
      channel: options?.useCodingAgent ? 'claude_code' : 'chat',
      initiator: 'user',
      eventsLoaded: true,
    }));
  }
  addPendingMessage(threadId, message, eventId, imageHashes);

  const body: ChatRequestBody = {
    message,
    mode: 'human',
    // Per-thread memory: send an explicit model ONLY when there's an override —
    // a compose first-send (`modelOverride`) or this thread's pending pick.
    // Otherwise omit it so the backend reuses the thread's last recorded model
    // (`resolve_chat_overrides_for_thread`), falling back to the account default
    // for a brand-new thread. Sending `currentModel` here was the bug: it forced
    // the global default onto every follow-up.
    model: options?.modelOverride ?? getThreadModelOverride(threadId).model,
    device_id: getDeviceId(),
    // reasoning_effort is set below: chat threads send it only when there's an
    // override or a per-thread pick (else the backend reuses the thread's last
    // effort); CC threads only set it when the user has a pending pick. Neither
    // may carry a stray default — for chat that would re-break per-thread memory,
    // and for CC the backend resolves from the prior session /
    // CodingAgentSettingsChanged events.
    event_id: eventId,
    thread_id: threadId,
    ...(options?.context ?? {}),
  };
  if (imageHashes?.length) body.image_hashes = imageHashes;
  // A raw new send mints its own thread id above (`threadId = ... || eventId`),
  // so the engine has never seen it. Say so: an unknown id with no create
  // signal is a 404 rather than a thread conjured out of whatever the caller
  // typed. `threadBeforeSend` is the PRE-insert snapshot, so it is undefined
  // exactly on the raw-new path; compose first-sends and follow-ups both pass
  // an explicit `threadId` for a thread that is already in the map, and a
  // compose thread's row exists server-side because `sendCompose` awaits
  // `POST /threads` first.
  if (!threadBeforeSend) body.new_thread = true;

  // `sendCompose` (compose.ts) flips composing→active before delegating here,
  // so by this point `threadBeforeSend.meta.state` is always 'active' when the
  // thread exists. The only discriminator that matters is whether the thread
  // is in `threadMap` at all — captured pre-insert because the optimistic
  // insert below creates a state='active' row for raw new sends.
  const isCcThread = threadBeforeSend
    ? threadBeforeSend.meta.channel === 'claude_code'
    : !!options?.useCodingAgent;
  if (isCcThread) {
    body.use_coding_agent = true;
    // Backend selection. Compose-promoted threads carry the binding on meta
    // (set in sendCompose); loaded follow-ups carry the server's stored value
    // (thread summary `coding_agent`); raw-new sends read the picker signal
    // directly (no thread to carry the binding). Omitting the field is always
    // safe — the engine resolves from `thread_summaries.coding_agent`.
    const requestedAgent = threadBeforeSend
      ? threadBeforeSend.meta.codingAgent
      : selectedCodingAgent.value;
    if (requestedAgent && requestedAgent !== 'claude-code') {
      body.coding_agent = requestedAgent;
    }
    // First send from compose-view: derive `folder` from the scope picker so
    // the engine routes via `coding_agent_kind` (Lucidos / app / external).
    // Follow-up on an existing thread: prefer the bound `codingAgentFolder`
    // when present (app threads), then fall back to `repoId` for back-compat
    // on threads bound before this rename. The engine resolves the absent
    // case via `thread_summaries.cc_repo_id` lookup.
    if (!threadBeforeSend) {
      const folder = scopeToFolder(selectedScope.value);
      if (folder) body.folder = folder;
    } else if (threadBeforeSend.meta.codingAgentFolder
      && threadBeforeSend.meta.codingAgentKind === 'app') {
      // Re-send the workspace-relative form the spawn was created with so
      // the engine's classifier lands on `App` again on every follow-up.
      body.folder = `data/apps/${threadBeforeSend.meta.codingAgentFolder.split('/').pop()}`;
    } else if (threadBeforeSend.meta.repoId) {
      body.repo_id = threadBeforeSend.meta.repoId;
    }
    // Apply the CC model/effort pick. A compose first-send passes the DRAFT's
    // resolved override (per-draft; `undefined` never reaches here from
    // sendCompose — it resolves to a value or null); raw-new sends + follow-ups
    // pass nothing, so we fall back to the global `codingAgentPending*` (the
    // active-thread control menu's per-thread pending, reconciled by
    // loadCommands). `null` = the explicit "default" pick → omit cc_model so the
    // backend resolves its own default. For an active follow-up we deliberately
    // do NOT clear the global pending here — it stays visible until
    // loadCommands() confirms the session adopted it (matched value), avoiding
    // the race where a stale in-flight fetch clears the user's pick.
    const ccModel = options?.ccModelOverride !== undefined
      ? options.ccModelOverride
      : codingAgentPendingModel.value;
    if (ccModel !== null) {
      body.cc_model = ccModel;
    }
    const ccEffort = options?.ccReasoningEffortOverride !== undefined
      ? options.ccReasoningEffortOverride
      : codingAgentPendingReasoningEffort.value;
    if (ccEffort !== null) {
      body.reasoning_effort = ccEffort;
    }
    // No CC pick and a CC thread → omit reasoning_effort entirely so the
    // backend falls through cc_reasoning_effort → prev_effort (live session)
    // → event_effort (CodingAgentSettingsChanged) → cc_default. The chat
    // default ('high') is a chat preference, not a CC preference, and would
    // wrongly override the prior session's effort on a follow-up after the
    // user already picked something else mid-session.
  } else {
    // Chat thread: an explicit override or this thread's pending pick; otherwise
    // omit so the backend reuses the thread's last effort (?? account default).
    const effort = options?.reasoningEffortOverride ?? getThreadModelOverride(threadId).reasoningEffort;
    if (effort) body.reasoning_effort = effort;
  }

  // CC ignores url_context; only send for non-CC threads. Content extraction is
  // tauri-only (browser can't read cross-origin iframes); fall back to URL+title.
  if (panelUrl.value && !body.use_coding_agent) {
    let extractedTitle: string | undefined;
    let extractedContent = '';
    if (isTauri()) {
      try {
        const res = await getWebviewContent();
        extractedTitle = res.title || undefined;
        extractedContent = res.content.trim() ? res.content : '';
      } catch { /* fall through with URL+title only */ }
    }
    body.url_context = {
      url: panelUrl.value,
      title: extractedTitle || panelTitle.value || undefined,
      content: extractedContent,
    };
  }
  try {
    // Serialized per thread: the optimistic row is already on screen, so
    // waiting for the thread's previous POST costs the user nothing visible and
    // is what keeps the engine's record in the order they pressed send.
    if (sendSlot.waitForTurn) await sendSlot.waitForTurn;
    // This body claims `mode: 'human'`, which the engine accepts only from a
    // device it can resolve, so the send must not overtake its own startup
    // registration. `null` on every send but the very first, and skipping the
    // await is what keeps a lone send synchronous (same reason as the chain
    // above). See `pendingDeviceRegistration`.
    const pendingRegistration = pendingDeviceRegistration();
    if (pendingRegistration) await pendingRegistration;
    await submitChat(body);
    schedulePendingCleanup(threadId, eventId);
    // The pick (if any) is now stamped on the sent message and becomes the
    // thread's remembered value; drop the ephemeral pending override so future
    // resolves come from the thread's events (no-op for CC / no pick).
    clearThreadModelOverride(threadId);
  } catch (error: unknown) {
    if (isTransportError(error)) {
      // Engine unreachable. Render the user's message as a failed in-thread
      // exchange (toast alone hides the text they spent time writing).
      // Passing eventId to handleEvent piggybacks on its pending-message
      // cleanup so the optimistic row inserted by addPendingMessage clears
      // without a second signal write via removePendingMessage.
      const messageSeq = -Date.now() - 1;
      const failedSeq = messageSeq + 1;
      const now = new Date().toISOString();
      handleEvent(threadMap.value, threadId, messageSeq, {
        type: 'MessageReceived',
        text: message,
      } as StoredEvent, now, eventId);
      handleEvent(threadMap.value, threadId, failedSeq, {
        type: 'ResponseFailed',
        error: getUnreachableEngineMsg(),
      } as StoredEvent, now);
      threadMap.value = new Map(threadMap.value);
      // Per `addPendingMessage`: focused-thread computeds subscribe to the
      // per-thread bump, not `threadMap`. Without this, the synthetic
      // MessageReceived + ResponseFailed events land in `thread.events` but
      // `activeExchanges` keeps its cached value until the next SSE event.
      bumpThreadEvents(threadId);
      return;
    }
    // HTTP error (4xx/5xx with body) or unknown bug. Raw new sends create
    // the thread optimistically (`threadBeforeSend === undefined`); the
    // engine has no record of it, so leaving the row would render a phantom
    // in the Active drawer that vanishes on refresh. Drop row + nav entries
    // and unfocus. Established threads keep their row; their content
    // predates this send and only the pending entry rolls back.
    if (threadBeforeSend === undefined) {
      const next = new Map(threadMap.value);
      next.delete(threadId);
      threadMap.value = next;
      // One of the two paths that remove a row outright (the other is
      // `rollbackOptimistic` in compose.ts), so it owes the thread-events
      // failure maps the same cleanup: nothing will ever fetch this thread
      // again to clear an entry keyed on it.
      forgetThreadEventsFailures(threadId);
      removeThreadNavEntries(threadId);
      if (focusedThreadId.value === threadId) setFocusedThread(null);
    } else {
      removePendingMessage(threadId, eventId);
    }
    showToast(`Failed to send message: ${errorDetail(error)}`, 'error');
  } finally {
    // Whatever happened to this POST, the next send on the thread may go. A
    // throw between `enterSendChain` and here would skip this, which is why
    // the wait is bounded rather than open-ended.
    sendSlot.release();
  }
}

/** Outcome of a Cancel/Stop click:
 *   - 'canceled' — the server canceled live work (or settled a stuck
 *     projection); a terminal event is on its way over SSE.
 *   - 'noop'     — the server had nothing to cancel (`{"canceled": false}`);
 *     the client's optimistic "canceling" state is stale and must be
 *     reconciled by re-syncing the thread.
 *   - 'failed'   — the API call itself failed (a toast was already shown). */
export type CancelOutcome = 'canceled' | 'noop' | 'failed';

/** The thread's queued (un-injected) chat follow-ups in FIFO order — the set a
 *  user Stop returns to compose. Chat-only: CC/Codex follow-ups go to stdin and
 *  are never queued. Derived from the same `queuedFollowupRun` the UI renders
 *  "Queued" bubbles from, so Stop clears exactly what the user saw queued. */
function getQueuedMessages(threadId: string): QueuedMessage[] {
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.meta.channel === 'claude_code') return [];
  const status = effectiveThreadStatus(thread);
  const threadBusy = status === 'running' || status === 'waiting_for_user_answer';
  // Exclude messages already being trashed — the UI hides them from the queued
  // group the same way (CreateThreadView `removedQueuedIndices`), so Stop clears
  // exactly what the user still sees queued and never resurfaces a just-trashed
  // message into compose (its own removal already owns it).
  const removing = removingQueuedMessageIds.value;
  return queuedMessagesFromExchanges(computeExchanges(thread), threadBusy, false)
    .filter(q => !removing.has(queuedMessageRemovalKey(threadId, q.id)));
}

/** Append retracted queued-message texts (FIFO) to the thread's compose draft,
 *  after any existing draft (blank-line separated), and force the prompt input
 *  to show it (the compose→textarea sync skips a focused non-empty input, so a
 *  programmatic append needs the explicit override — as `applySuggestion` does). */
function appendQueuedTextToCompose(threadId: string, texts: string[]): void {
  if (texts.length === 0) return;
  const existing = getDraft(threadId).text;
  const addition = texts.join('\n\n');
  const combined = existing.trim().length > 0 ? `${existing}\n\n${addition}` : addition;
  updateCompose(threadId, { text: combined });
  requestPromptOverrideSync();
}

/** On a user Stop of a chat thread, return un-injected queued follow-ups to the
 *  compose box instead of letting them re-run as a new response after the cancel
 *  (the bug where a queued message streamed above "Response canceled"). Retracts
 *  each via the `QueuedMessageRemoved` tombstone so the backend's
 *  `filter_removed_queued_prompts` drops it at loop finalize — see
 *  `docs/plans/2026-07-19-stop-clears-queued-messages.md`. MUST run BEFORE
 *  `cancelChat` so the tombstones persist before the loop finalizes. Messages
 *  the loop already injected (409) stay under the cancelled exchange and are NOT
 *  moved to compose. */
async function clearQueuedMessagesToCompose(threadId: string): Promise<void> {
  const queued = getQueuedMessages(threadId);
  if (queued.length === 0) return;
  const removedTexts: string[] = [];
  for (const q of queued) {
    const { outcome } = await retractQueuedMessage(threadId, q.id);
    if (outcome === 'removed') removedTexts.push(q.text);
    // 'already-injected' → now part of the cancelled response; 'failed' →
    // stays queued (user can trash it). Neither goes to compose.
  }
  appendQueuedTextToCompose(threadId, removedTexts);
}

/**
 * Cancel a thread's in-flight exchange. Routes to the chat or CC endpoint
 * based on thread channel. Pinning the threadId at call time matters: the
 * user can switch focus between clicking Cancel and the API resolving, and
 * we must not cancel the wrong thread.
 */
export async function cancelCurrentExchange(threadId?: string): Promise<CancelOutcome> {
  const tid = threadId ?? focusedThreadId.value ?? undefined;
  try {
    const thread = tid ? threadMap.value.get(tid) : undefined;
    if (thread?.meta.channel === 'claude_code') {
      const canceled = await stopClaudeCode(undefined, tid);
      return canceled ? 'canceled' : 'noop';
    }
    // Chat (Lucidos Agent): return any un-injected queued follow-ups to compose
    // and retract them BEFORE cancelling, so they don't re-run as a new response
    // above the "Response canceled" marker. Best-effort — a queue-clear hiccup
    // must never block the actual cancel (a still-queued message stays visible
    // and trashable; telemetry carve-out per .claude/rules/frontend.md).
    if (tid) {
      try {
        await clearQueuedMessagesToCompose(tid);
      } catch (e) {
        console.warn('[cancel] failed to clear queued messages to compose', e);
      }
    }
    const canceled = await cancelChat(tid);
    return canceled ? 'canceled' : 'noop';
  } catch (err) {
    showToast(`Failed to cancel: ${errorDetail(err)}`, 'error');
    return 'failed';
  }
}

/**
 * Set the optimistic "canceling" flag for a thread, fire the cancel API, and
 * reconcile the flag by outcome:
 *   - 'canceled': keep the flag — PromptInput's status-transition effect
 *     (`shouldClearCanceling`) releases it once the thread leaves mid-turn.
 *   - 'noop': the server had nothing to cancel, so no terminal event will ever
 *     arrive to release the flag — the exact wedge that leaves Cancel disabled
 *     while the thread visibly keeps going. Release the flag now AND re-sync the
 *     thread (`refreshThreadEvents`) so any terminal event the client missed
 *     (e.g. a `ResponseCanceled` broadcast the page raced on load) lands and the
 *     status snaps to truth.
 *   - 'failed': roll the flag back so the user can retry (toast already shown).
 */
export async function handleCancelExchange(threadId: string): Promise<void> {
  const next = new Set(cancelingThreadIds.value);
  next.add(threadId);
  cancelingThreadIds.value = next;
  const outcome = await cancelCurrentExchange(threadId);
  if (outcome === 'canceled') return;
  const rollback = new Set(cancelingThreadIds.value);
  rollback.delete(threadId);
  cancelingThreadIds.value = rollback;
  setCanceledQuestion(threadId, undefined);
  setCanceledWhileAwaiting(threadId, false);
  if (outcome === 'noop') {
    // Stale view: re-read events + currentAggregate so the missed terminal
    // event lands and the thread stops looking mid-turn.
    void refreshThreadEvents(threadId);
  }
}
