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
import { submitChat, cancelChat, stopClaudeCode, removeQueuedMessage as removeQueuedMessageRequest, ApiError, type CodingAgentModelValue, type CodingAgentReasoningEffort } from '../../api/client';
import { sendFailureOf, withQuietRetries, type SendFailure } from './sendRetry';
import { enterSendChain, type SendSlot } from './sendChain';
import { getDeviceId } from './devices';
import { endUnsentMessage, recordUnsentMessage, takeUnsentMessage, unsentMessages, type SendSettlement } from '../unsentMessages';
import { forgetUnsentMessageRecord, markUnsentMessageRecordUnsent, persistSendingMessage, type UnsentMessageRecord } from '../unsentMessageRecords';
import { restoreRefusedSend, settleAcceptedSend } from './sendSettlement';
import { unsentPicks } from '../pendingDecisions';
import { generateUuid } from '../../utils/uuid';
import { handleEvent, makeOptimisticThreadState, computeExchanges, queuedMessagesFromExchanges, retireUnsentExchange, type PendingUserMessage, type StoredEvent, type QueuedMessage } from '../thread-events';
import { questionTheSendAnswers } from '../../components/chat/prompt-input-helpers';
import { getDraft } from '../composeDrafts';
import { reopenEmptyDraft, updateCompose } from './compose';
import { requestPromptOverrideSync } from '../../components/chat/promptValueSync';
import { focusPromptNow } from '../../components/chat/promptFocus';
import { bumpThreadEvents } from '../threadActivity';
import { getThreadModelOverride, clearThreadModelOverride } from '../threadModelSelections';
import { pushThreadNavState, removeThreadNavEntries } from './thread-navigation';
import { formatThreadLabel } from './thread-label';
import { revealThreadPane } from './pane';
import { retireWelcomeAfterUse } from './preferences';
import { followSentMessage } from '../../components/chat/scrollState';
import { setCanceledQuestion, setCanceledWhileAwaiting } from '../canceledQuestions';
import { refreshThreadEvents, forgetThreadEventsFailures } from './thread-loading';
import { markThreadRerenderStart } from '../../utils/threadOpenMarks';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { isTauri } from '../../utils/platform';
import { getWebviewContent } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';

/** Safety timeout (ms) for an accepted send's pending row. If SSE doesn't
 *  deliver the MessageReceived event within this window, we force-refresh
 *  thread events and clear that row. Prevents "Requesting..." getting
 *  stuck indefinitely when SSE drops after submitChat() succeeds. */
export const PENDING_MESSAGE_SAFETY_MS = 30_000;

type RemovedPendingMessage = {
  index: number;
  message: PendingUserMessage;
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
  reportFailedRetract(threadId, 'remove', error);
}

/** Edit a queued message: take it back, then put its text and images in the
 *  compose box after any draft. Compose changes only once the removal took, so
 *  a message the agent already read never also sits in compose. */
export async function editQueuedMessage(threadId: string, message: QueuedMessage): Promise<void> {
  const { outcome, error } = await retractQueuedMessage(threadId, message.id);
  if (outcome !== 'removed') {
    reportFailedRetract(threadId, 'edit', error);
    return;
  }
  appendMessagesToCompose(threadId, [message]);
  focusPromptNow();
}

/** A transport error, or a 409 because the agent read the message just now.
 *  Re-sync so the row shows the truth, and say why it did not take. */
function reportFailedRetract(threadId: string, action: 'remove' | 'edit', error: unknown): void {
  void refreshThreadEvents(threadId);
  showToast(`Failed to ${action} queued message: ${errorDetail(error)}`, 'error');
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
 *  force-refresh thread events and clear this send's pending row.
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
      // Only this send's row: another may still be sending or waiting its turn,
      // and its age says nothing about whether it was accepted.
      removePendingMessage(threadId, eventId);
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
      // proposes a change, inside `inFlightThreadCount`, and showing a Stop with
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

/** Add an optimistic pending message to a thread so the user sees it
 *  immediately. Returns the question card the message answers, if any. */
function addPendingMessage(
  threadId: string,
  message: string,
  eventId: string,
  imageHashes?: string[],
): string | undefined {
  const target = threadMap.value.get(threadId);
  // Read before the push: the new row flips the thread to running.
  const answersQuestion = target ? questionTheSendAnswers(target) : undefined;
  if (answersQuestion) discardUnsentAnswers(threadId, answersQuestion);
  const map = threadMap.value;
  const thread = map.get(threadId);
  if (thread) {
    thread.pendingUserMessages.push({
      text: message,
      eventId,
      created: new Date().toISOString(),
      image_hashes: imageHashes,
      ...(answersQuestion ? { answersQuestion } : {}),
    });
    if (focusedThreadId.value === threadId) {
      // The reader just produced the content at the bottom, so rest them on the
      // live edge until the agent draws (ADR 0080). It arms nothing. Called
      // BEFORE the threadMap write below. The optimistic row has therefore not
      // rendered, so the landing waits for it rather than resolving the previous
      // message. That ordering also makes a BRAND-NEW thread read as the empty
      // transcript it still is. So `followSentMessage` knows this send has
      // nowhere to take anybody.
      followSentMessage();
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
  return answersQuestion;
}

/** A new answer to a question card replaces any typed answer to it that got
 *  no answer. Called for every way this device answers a card. */
export function discardUnsentAnswers(threadId: string, toolUseId: string): void {
  for (const [eventId, unsent] of unsentMessages.value) {
    if (unsent.threadId === threadId && unsent.answersQuestion === toolUseId) discardUnsentMessage(eventId);
  }
}

// `loadRepositories` lives in `./repositoriesLoader` so SSE-handler modules
// can refresh the repositories cache without dragging in chat.ts's transitive
// import tree. Re-exported here so existing call sites
// (`import { loadRepositories } from '../store/actions/chat'`) keep working.
export { loadRepositories } from './repositoriesLoader';

/** What became of a send's text.
 *   - 'sent': the engine accepted it.
 *   - 'shown-as-failed': no attempt got an answer, so the text stays in the
 *     thread as an unsent message with a Retry (`showUnsentExchange`).
 *   - 'dropped': the engine refused it and the optimistic row is gone. A toast
 *     was shown, and the caller owns putting the text back. */
export type SendOutcome = 'sent' | 'shown-as-failed' | 'dropped';

/**
 * Send a chat message. Side effects (modals, refreshes) are handled by
 * thread-sync.ts via ThreadEvent SSE events, with no listener registry.
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
    providerOverride?: string;
    ccModelOverride?: CodingAgentModelValue | null;
    ccReasoningEffortOverride?: CodingAgentReasoningEffort | null;
    /** Which kind of send this is, for a decision after the first attempt.
     *  Absent means a follow-up, or a raw new send when it creates its thread. */
    settlement?: SendSettlement;
    /** Resolves once the engine holds the thread row. Awaited after the
     *  optimistic row, so the thread reads as running while it is pending. */
    threadStarted?: () => Promise<void>;
  },
): Promise<SendOutcome> {
  threadsLoaded.value = true;
  const eventId = generateUuid();
  const explicitThreadId = options?.threadId;
  const shouldFocus = options?.focus ?? true;
  const isNewThread = explicitThreadId === undefined && focusedThreadId.value === null;
  const threadId = explicitThreadId || focusedThreadId.value || eventId;
  // Claim the chain slot before ANY await below, so this send's place in the
  // thread's order is the order the user pressed send. Released however the
  // send ends, a throw before its POST included.
  const sendSlot = enterSendChain(threadId);
  try {
    return await buildAndPostSend({ message, imageHashes, options, eventId, threadId, isNewThread, shouldFocus, sendSlot });
  } catch (error) {
    // A throw before the POST: no sweep will come for this row, and a pending
    // row counts as a turn in flight.
    removePendingMessage(threadId, eventId);
    forgetUnsentMessageRecord(eventId);
    throw error;
  } finally {
    sendSlot.release();
  }
}

type SendMessageOptions = NonNullable<Parameters<typeof sendMessage>[2]>;

/** Everything `sendMessage` does once its chain slot is claimed: the
 *  optimistic row, the request body, and the POST. */
async function buildAndPostSend({ message, imageHashes, options, eventId, threadId, isNewThread, shouldFocus, sendSlot }: {
  message: string;
  imageHashes: string[] | undefined;
  options: SendMessageOptions | undefined;
  eventId: string;
  threadId: string;
  isNewThread: boolean;
  shouldFocus: boolean;
  sendSlot: SendSlot;
}): Promise<SendOutcome> {
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
  const answersQuestion = addPendingMessage(threadId, message, eventId, imageHashes);
  // A typed answer replaces a pick to the same card that was not sent.
  if (answersQuestion) unsentPicks.clear(answersQuestion);
  // After the row, never before: a send in flight must have one. The Stop
  // bridge (`shouldClearSubmitting`) and the composer's action row read it.
  if (options?.threadStarted) await options.threadStarted();

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
  // compose thread's row exists server-side because `threadStarted` was
  // awaited above.
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
    // Same rule for the backend: only an explicit pick is sent, so the thread's
    // memory and then the model's own default decide otherwise.
    const provider = options?.providerOverride ?? getThreadModelOverride(threadId).provider;
    if (provider) body.provider = provider;
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
  const settlement: SendSettlement = options?.settlement ?? { kind: body.new_thread ? 'raw-new' : 'follow-up' };
  // Kept on the device before the POST goes out, so a reload while it is in
  // flight, or waiting its turn, still finds the message.
  persistSendingMessage({ eventId, threadId, body, settlement, sentAt: new Date().toISOString(), answersQuestion });
  return postSend({ threadId, eventId, body, sendSlot, failedRetries: 0, settlement });
}

/** The POST of a send whose optimistic row is already on screen, with its quiet
 *  retries. Shared by the first attempt (`sendMessage`) and every Retry
 *  (`retryUnsentMessage`), so both settle the same three ways. The row stays
 *  in its sending state until the last attempt. The caller owns the slot. */
async function postSend(send: {
  threadId: string;
  eventId: string;
  body: ChatRequestBody;
  sendSlot: SendSlot;
  failedRetries: number;
  settlement: SendSettlement;
}): Promise<SendOutcome> {
  const { threadId, eventId, body, sendSlot } = send;
  // Read before the POST: the engine's answer clears the pending row on arrival.
  const answersQuestion = threadMap.value.get(threadId)?.pendingUserMessages
    .find(p => p.eventId === eventId)?.answersQuestion;
  const accepted = (): SendOutcome => {
    forgetUnsentMessageRecord(eventId);
    schedulePendingCleanup(threadId, eventId);
    // The pick (if any) is now stamped on the sent message and becomes the
    // thread's remembered value; drop the ephemeral pending override so future
    // resolves come from the thread's events (no-op for CC / no pick).
    clearThreadModelOverride(threadId);
    void retireWelcomeAfterUse(threadMap.value.values());
    return 'sent';
  };
  // Serialized per thread. The optimistic row is already on screen, so the
  // wait costs the user nothing visible. It keeps the engine's record in the
  // order they pressed send.
  if (sendSlot.waitForTurn) await sendSlot.waitForTurn;
  const result = await withQuietRetries(() => submitChat(body), {
    path: 'message',
    // The engine's own row can beat a lost answer over SSE. The send landed,
    // so it is accepted, and a retry or an unsent card would be a duplicate.
    landed: () => engineRecordedMessage(threadId, eventId, answersQuestion ? { toolUseId: answersQuestion, text: body.message } : false),
  });
  switch (result.kind) {
    case 'done':
    case 'landed':
      return accepted();
    case 'gave-up':
      // No answer, so the engine may never have seen it. Keep the text in the
      // thread as an unsent message with a Retry; a toast alone would hide it.
      showUnsentExchange({ ...send, failure: sendFailureOf(result) });
      markUnsentMessageRecordUnsent(eventId, send.failedRetries);
      return 'shown-as-failed';
    case 'refused':
      // HTTP error (4xx/5xx with body) or unknown bug. A raw new send created
      // its thread optimistically (`new_thread`), and the engine has no record
      // of it. A row left behind would be a phantom in the Active drawer that
      // vanishes on refresh. So drop row + nav entries and unfocus. Established
      // threads keep their row; their content predates this send and only the
      // pending entry rolls back.
      if (body.new_thread) {
        dropNeverMadeThread(threadId);
      } else {
        removePendingMessage(threadId, eventId);
      }
      forgetUnsentMessageRecord(eventId);
      showToast(`Failed to send message: ${errorDetail(result.error)}`, 'error');
      return 'dropped';
  }
}

/** Remove the optimistic row of a thread the engine never made, and every
 *  entry keyed on it. One of the paths that remove a row outright (another is
 *  `rollbackOptimistic` in compose.ts). Nothing will ever fetch this thread
 *  again to clear an entry keyed on it. */
function dropNeverMadeThread(threadId: string): void {
  const next = new Map(threadMap.value);
  next.delete(threadId);
  threadMap.value = next;
  forgetThreadEventsFailures(threadId);
  removeThreadNavEntries(threadId);
  if (focusedThreadId.value === threadId) setFocusedThread(null);
}

/** Does the thread already hold the engine's own row for this message? A
 *  typed answer to a question card leaves no row with its event id: the engine
 *  records it as that card's `FreeText` answer, matched here by its text. */
export function engineRecordedMessage(
  threadId: string,
  eventId: string,
  answer?: { toolUseId: string; text: string } | false,
): boolean {
  const thread = threadMap.value.get(threadId);
  if (!thread) return false;
  for (const [seq, stored] of thread.events) {
    if (seq <= 0) continue;
    if (stored._eventId === eventId) return true;
    if (answer && stored.type === 'UserQuestionAnswered' && stored.tool_use_id === answer.toolUseId
      && stored.answer.kind === 'FreeText' && stored.answer.text === answer.text) return true;
  }
  return false;
}

/** What an unsent message's card says. */
export function unsentMessageCopy(failedRetries: number): string {
  return failedRetries === 0
    ? 'Lucidos did not answer, so this message was not sent.'
    : 'Lucidos still did not answer. Try again in a moment.';
}

/** The card for a send the page reloaded away from before Lucidos answered. */
export const UNSENT_AFTER_RELOAD_COPY = 'The page reloaded before Lucidos answered, so this message may not have been sent.';

/** The next free client-only seq. Counting down keeps every unsent pair on its
 *  own two seqs; a clock-derived seq let two sends a millisecond apart overlap. */
let nextUnsentSeq = -1;

/** Swap a send's optimistic row for an unsent exchange: its message plus a
 *  `ResponseFailed`, both client-only on negative seqs, and record what Retry
 *  needs. `handleEvent` drops the pair if the engine's own row for this event
 *  id turns up after all. */
function showUnsentExchange(send: {
  threadId: string;
  eventId: string;
  body: ChatRequestBody;
  failedRetries: number;
  settlement: SendSettlement;
  /** When it was sent, for one restored after a reload. Exchanges sort by it. */
  sentAt?: string;
  copy?: string;
  /** The question card it answers, for one restored after a reload. A live
   *  send carries it on its pending row instead. */
  answersQuestion?: string;
  failure?: SendFailure;
}): void {
  const { threadId, eventId, body } = send;
  const failedSeq = nextUnsentSeq;
  const messageSeq = failedSeq - 1;
  nextUnsentSeq -= 2;
  const now = send.sentAt ?? new Date().toISOString();
  const answersQuestion = threadMap.value.get(threadId)?.pendingUserMessages
    .find(p => p.eventId === eventId)?.answersQuestion ?? send.answersQuestion;
  // Passing eventId piggybacks on handleEvent's pending-message cleanup, so the
  // optimistic row clears without a second write via removePendingMessage.
  handleEvent(threadMap.value, threadId, messageSeq, {
    type: 'MessageReceived',
    text: body.message,
    user_image_hashes: body.image_hashes,
    _unsent: true,
    ...(answersQuestion ? { _answersQuestion: answersQuestion } : {}),
  } as StoredEvent, now, eventId);
  handleEvent(threadMap.value, threadId, failedSeq, {
    type: 'ResponseFailed',
    error: send.copy ?? unsentMessageCopy(send.failedRetries),
    _unsent: true,
  } as StoredEvent, now);
  const thread = threadMap.value.get(threadId);
  if (thread) (thread.unsentMessageSeqs ??= new Map()).set(eventId, messageSeq);
  threadMap.value = new Map(threadMap.value);
  // Per `addPendingMessage`: focused-thread computeds subscribe to the
  // per-thread bump, not `threadMap`.
  bumpThreadEvents(threadId);
  recordUnsentMessage(eventId, {
    threadId,
    body,
    failedRetries: send.failedRetries,
    settlement: send.settlement,
    ...(answersQuestion ? { answersQuestion } : {}),
    ...(send.failure ? { failure: send.failure } : {}),
  });
}

/** Show an unsent message a previous page load left, with its Retry, where
 *  it was sent. */
export function showRestoredUnsentMessage(record: UnsentMessageRecord): void {
  showUnsentExchange({
    ...record,
    copy: record.phase === 'sending' ? UNSENT_AFTER_RELOAD_COPY : unsentMessageCopy(record.failedRetries),
  });
}

/** Re-post an unsent message: the same request, event id included, so an
 *  engine that did get the first one acks without running it twice. The card
 *  turns back into a pending row while the POST runs, and an answer typed to a
 *  question card goes back to sending on that card. Resolves null when there
 *  is nothing to retry, e.g. a second press. */
export async function retryUnsentMessage(eventId: string): Promise<SendOutcome | null> {
  const unsent = takeUnsentMessage(eventId);
  if (!unsent) return null;
  const { threadId, body } = unsent;
  const thread = threadMap.value.get(threadId);
  if (!thread) return null;
  const sendSlot = enterSendChain(threadId);
  let outcome: SendOutcome;
  try {
    retireUnsentExchange(thread, eventId);
    addPendingMessage(threadId, body.message, eventId, body.image_hashes);
    outcome = await postSend({
      threadId,
      eventId,
      body,
      sendSlot,
      failedRetries: unsent.failedRetries + 1,
      settlement: unsent.settlement,
    });
  } finally {
    sendSlot.release();
  }
  if (outcome === 'sent') settleAcceptedSend(threadId, unsent.settlement);
  if (outcome === 'dropped') restoreRefusedSend(threadId, body, unsent.settlement);
  return outcome;
}

/** Drop an unsent message the user no longer wants sent: its card and its
 *  stored copy. Nothing is posted, and nothing is put back in a draft. */
export function discardUnsentMessage(eventId: string): void {
  const unsent = endUnsentMessage(eventId);
  if (!unsent) return;
  const thread = threadMap.value.get(unsent.threadId);
  if (!thread || !retireUnsentExchange(thread, eventId)) return;
  const nothingElseSent = thread.events.size === 0 && thread.pendingUserMessages.length === 0;
  // A raw new send's thread exists only on this device, so it goes too.
  if (unsent.settlement.kind === 'raw-new' && nothingElseSent) {
    dropNeverMadeThread(unsent.threadId);
    return;
  }
  if (unsent.settlement.kind === 'first-send' && nothingElseSent) reopenEmptyDraft(unsent.threadId);
  threadMap.value = new Map(threadMap.value);
  bumpThreadEvents(unsent.threadId);
}

/** Outcome of a Cancel/Stop click:
 *   - 'canceled' — the server canceled live work (or settled a stuck
 *     projection); a terminal event is on its way over SSE.
 *   - 'noop'     — the server had nothing to cancel (`{"canceled": false}`);
 *     the client's optimistic "canceling" state is stale and must be
 *     reconciled by re-syncing the thread.
 *   - 'failed'   — the API call itself failed (a toast was already shown). */
export type CancelOutcome = 'canceled' | 'noop' | 'failed';

/** The thread's queued (un-injected) chat follow-ups in FIFO order: the set a
 *  user Stop returns to compose. Chat-only: a coding agent keeps its queued
 *  messages across a Stop, so only the bin or Edit takes one back. Derived from
 *  the same `queuedFollowupRun` the UI renders "Queued" bubbles from, so Stop
 *  clears exactly what the user saw queued. */
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

/** Append taken-back messages (FIFO) to the thread's compose draft, after any
 *  existing draft: retracted queued messages, or a refused follow-up. Texts
 *  join with a blank line, and images go after the draft's own, each hash
 *  once, as `addAttachedImageHash` keeps it. Forces the prompt input to show
 *  the text. The compose→textarea sync skips a focused non-empty input, so a
 *  programmatic append needs the explicit override, as `seedSuggestion` does. */
export function appendMessagesToCompose(threadId: string, messages: ReadonlyArray<{ text: string; imageHashes: readonly string[] }>): void {
  const draftImages = getDraft(threadId).image_hashes;
  const images = [...new Set(messages.flatMap(m => m.imageHashes))].filter(h => !draftImages.includes(h));
  if (images.length > 0) {
    updateCompose(threadId, { image_hashes: [...draftImages, ...images] });
  }
  const texts = messages.map(m => m.text).filter(t => t.length > 0);
  if (texts.length === 0) return;
  const existing = getDraft(threadId).text;
  const addition = texts.join('\n\n');
  const combined = existing.trim().length > 0 ? `${existing}\n\n${addition}` : addition;
  updateCompose(threadId, { text: combined });
  // `'append'` rather than `'replace'`: the draft the user was writing keeps its
  // prefix, so their caret still points at the character it did and stays put.
  requestPromptOverrideSync('append');
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
  const removed: QueuedMessage[] = [];
  let failed = 0;
  for (const q of queued) {
    const { outcome } = await retractQueuedMessage(threadId, q.id);
    if (outcome === 'removed') removed.push(q);
    // 'already-injected' → now part of the cancelled response, so nothing is
    // owed. 'failed' → no tombstone persisted, so the loop will NOT drop it at
    // finalize. Neither goes to compose.
    if (outcome === 'failed') failed++;
  }
  appendMessagesToCompose(threadId, removed);
  // The user pressed Stop, so this is theirs to know. A retract that got no
  // tombstone leaves the follow-up queued, and it runs as a fresh response
  // after the cancel. That is the exact outcome this function exists to
  // prevent, so it cannot be silent. The row stays on screen and trashable.
  if (failed > 0) {
    showToast(
      `Stopped, but ${failed} queued message${failed > 1 ? 's' : ''} could not be retracted `
      + `in ${formatThreadLabel(threadId)}. They may still run.`,
      'error',
      { key: `queued-retract-${threadId}` },
    );
  }
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
