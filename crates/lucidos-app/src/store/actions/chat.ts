import {
  threadsLoaded,
  currentModel,
  reasoningEffort,
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
} from '../store';
import type { ChatContext } from './chatContext';
import type { ChatRequestBody } from '../../api/types';
import { submitChat, cancelChat, stopClaudeCode, isTransportError, removeQueuedMessage as removeQueuedMessageRequest } from '../../api/client';
import { getUnreachableEngineMsg } from './connection';
import { getDeviceId } from './devices';
import { handleEvent, makeOptimisticThreadState, type StoredEvent } from '../thread-events';
import { bumpThreadEvents } from '../threadActivity';
import { pushThreadNavState, removeThreadNavEntries } from './thread-navigation';
import { revealThreadPane } from './pane';
import { scrollToBottom } from '../../components/chat/scrollState';
import { refreshThreadEvents } from './thread-loading';
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

export async function removeQueuedMessage(threadId: string, messageId: string): Promise<void> {
  const key = queuedMessageRemovalKey(threadId, messageId);
  if (removingQueuedMessageIds.value.has(key)) return;

  removingQueuedMessageIds.value = new Set([...removingQueuedMessageIds.value, key]);
  const removedPending = removePendingMessage(threadId, messageId);

  try {
    await removeQueuedMessageRequest(threadId, messageId);
  } catch (err) {
    const next = new Set(removingQueuedMessageIds.value);
    next.delete(key);
    removingQueuedMessageIds.value = next;
    restorePendingMessage(threadId, removedPending);
    void refreshThreadEvents(threadId).catch(() => {});
    showToast(`Failed to remove queued message: ${errorDetail(err)}`, 'error');
  }
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
 *  follow-up. On a failed refetch we reschedule another attempt instead (the
 *  guard at the top exits once the pending is gone, so the retry is bounded by
 *  the pending's lifetime and self-clears when SSE catches up or any later
 *  refetch succeeds). */
export function schedulePendingCleanup(threadId: string, eventId: string): void {
  setTimeout(async () => {
    const thread = threadMap.value.get(threadId);
    if (!thread || !thread.pendingUserMessages.some(p => p.eventId === eventId)) return;
    let refetchOk = true;
    await refreshThreadEvents(threadId).catch(() => { refetchOk = false; });
    if (refetchOk) {
      clearStalePendingMessages(threadId);
    } else {
      // Transient failure — don't drop a persisted message; try again later.
      schedulePendingCleanup(threadId, eventId);
    }
  }, PENDING_MESSAGE_SAFETY_MS);

  // CC threads only: pick up terminal events (ResponseAborted) emitted during
  // CC cleanup that weren't ready at the 30s mark.
  const existing = threadMap.value.get(threadId);
  if (existing?.meta.channel === 'claude_code') {
    setTimeout(async () => {
      const thread = threadMap.value.get(threadId);
      if (!thread || thread.meta.status !== 'running') return;
      await refreshThreadEvents(threadId).catch(() => {});
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
  options?: { useCodingAgent?: boolean; context?: ChatContext | null; threadId?: string; focus?: boolean },
): Promise<void> {
  threadsLoaded.value = true;
  const eventId = crypto.randomUUID();
  const explicitThreadId = options?.threadId;
  const shouldFocus = options?.focus ?? true;
  const isNewThread = explicitThreadId === undefined && focusedThreadId.value === null;
  const threadId = explicitThreadId || focusedThreadId.value || eventId;

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
    model: currentModel.value,
    device_id: getDeviceId(),
    // reasoning_effort is set below: chat threads use the chat preference,
    // CC threads only set it when the user has a pending pick. CC follow-ups
    // without a pending pick must NOT carry the chat default — the backend
    // resolves from the prior session / CodingAgentSettingsChanged events,
    // and a stray default here overrides that with the unrelated chat value.
    event_id: eventId,
    thread_id: threadId,
    ...(options?.context ?? {}),
  };
  if (imageHashes?.length) body.image_hashes = imageHashes;

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
    // Apply pending CC preferences (set from compose view before session start).
    // Don't clear pending here — they stay visible in the UI until
    // loadCommands() confirms the session adopted them (matched value via
    // current_reasoning_effort / current_model). Clearing early causes a
    // race: loadCommands() fires before the session exists, gets stale cache
    // values, and with pending gone the UI shows the previous session's
    // effort/model instead of the user's selection.
    // Consumption: a compose first-send spawn is one-shot — sendCompose clears
    // the pick after this await so it can't leak onto the next new thread.
    // Follow-ups keep the pick until loadCommands reconciles it per-thread.
    if (codingAgentPendingModel.value !== null) {
      body.cc_model = codingAgentPendingModel.value;
    }
    if (codingAgentPendingReasoningEffort.value !== null) {
      body.reasoning_effort = codingAgentPendingReasoningEffort.value;
    }
    // No CC pending and a CC thread → omit reasoning_effort entirely so the
    // backend falls through cc_reasoning_effort → prev_effort (live session)
    // → event_effort (CodingAgentSettingsChanged) → cc_default. The chat
    // default ('high') is a chat preference, not a CC preference, and would
    // wrongly override the prior session's effort on a follow-up after the
    // user already picked something else mid-session.
  } else {
    body.reasoning_effort = reasoningEffort.value;
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
    await submitChat(body);
    schedulePendingCleanup(threadId, eventId);
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
      removeThreadNavEntries(threadId);
      if (focusedThreadId.value === threadId) setFocusedThread(null);
    } else {
      removePendingMessage(threadId, eventId);
    }
    showToast(`Failed to send message: ${errorDetail(error)}`, 'error');
  }
}

/**
 * Cancel a thread's in-flight exchange. Routes to the chat or CC endpoint
 * based on thread channel. Pinning the threadId at call time matters: the
 * user can switch focus between clicking Cancel and the API resolving, and
 * we must not cancel the wrong thread.
 * Returns false if the API call failed — caller resets optimistic UI.
 */
export async function cancelCurrentExchange(threadId?: string): Promise<boolean> {
  const tid = threadId ?? focusedThreadId.value ?? undefined;
  try {
    const thread = tid ? threadMap.value.get(tid) : undefined;
    if (thread?.meta.channel === 'claude_code') {
      await stopClaudeCode(undefined, tid);
    } else {
      await cancelChat(tid);
    }
    return true;
  } catch (err) {
    showToast(`Failed to cancel: ${errorDetail(err)}`, 'error');
    return false;
  }
}

/**
 * Set the optimistic "canceling" flag for a thread, fire the cancel API, and
 * roll back the flag on failure. Cleared on success by PromptInput's
 * status-transition effect once the thread leaves active status.
 */
export async function handleCancelExchange(threadId: string): Promise<void> {
  const next = new Set(cancelingThreadIds.value);
  next.add(threadId);
  cancelingThreadIds.value = next;
  const ok = await cancelCurrentExchange(threadId);
  if (!ok) {
    const rollback = new Set(cancelingThreadIds.value);
    rollback.delete(threadId);
    cancelingThreadIds.value = rollback;
  }
}
