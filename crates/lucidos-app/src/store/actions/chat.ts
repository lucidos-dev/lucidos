import {
  threadsLoaded,
  currentModel,
  reasoningEffort,
  currentApp,
  previewFile,
  selectedLines,
  panelUrl,
  panelTitle,
  isConnected,
  showToast,
  focusedThreadId,
  threadMap,
  repositories,
  selectedRepoId,
  ccPendingModel,
  ccPendingReasoningEffort,
  parseRepoPath,
  cancelingThreadIds,
  setFocusedThread,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import type { ChatRequestBody } from '../../api/types';
import { API_BASE, submitChat, cancelChat, cancelClaudeCode } from '../../api/client';
import { getDisconnectedMsg } from './connection';
import { getDeviceId } from './devices';
import { handleEvent, makeOptimisticThreadState, type StoredEvent } from '../thread-events';
import { pushThreadNavState } from './thread-navigation';
import { scrollToBottom } from '../../components/chat/scrollState';
import { refreshThreadEvents } from './thread-loading';
import { isTauri } from '../../utils/platform';
import { getWebviewContent } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';

/** Safety timeout (ms) for pending messages. If SSE doesn't deliver the
 *  MessageReceived event within this window, we force-refresh thread events
 *  and clear the stale pending message. Prevents "Requesting..." getting
 *  stuck indefinitely when SSE drops after submitChat() succeeds. */
export const PENDING_MESSAGE_SAFETY_MS = 30_000;

/** Remove an optimistic pending message from a thread.
 *  Without cleanup, the thread stays stuck in "Requesting..." forever
 *  (pendingUserMessages never cleared → effectiveThreadStatus returns 'running'). */
function removePendingMessage(threadId: string, eventId: string): void {
  const t = threadMap.value.get(threadId);
  if (!t) return;
  const idx = t.pendingUserMessages.findIndex(m => m.eventId === eventId);
  if (idx !== -1) {
    t.pendingUserMessages.splice(idx, 1);
    threadMap.value = new Map(threadMap.value);
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
 *  (e.g. ResponseAborted for lost follow-ups) that weren't ready at 30s. */
function schedulePendingCleanup(threadId: string, eventId: string): void {
  setTimeout(async () => {
    const thread = threadMap.value.get(threadId);
    if (!thread || !thread.pendingUserMessages.some(p => p.eventId === eventId)) return;
    await refreshThreadEvents(threadId).catch(() => {});
    clearStalePendingMessages(threadId);
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
    scrollToBottom();
    threadMap.value = new Map(map);
  }
}

/** Load registered repositories from the backend. */
export async function loadRepositories(): Promise<void> {
  setLoadingIfFresh(repositories);
  try {
    const res = await fetch(`${API_BASE}/api/repositories`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    repositories.value = { status: 'loaded', data };
  } catch (e) {
    repositories.value = toFailed(e);
  }
}

/**
 * Returns context based on the current state.
 * - If an app UI is open, includes app_context so the LLM knows which app is active
 * - If viewing a file, includes file_context
 * - claude_code mode: null (handled separately)
 */
function getActiveContext(): {
  app_context?: { app_id: string };
  file_context?: { path: string };
  repo_file_context?: { repo_id: string; path: string; lines?: [number, number] };
} | null {
  // If an app UI is open, let the LLM know
  const app = currentApp.value;
  if (app) {
    return {
      app_context: {
        app_id: app.id,
      },
    };
  }

  // Check file preview — repo files have "repo:" prefix
  const file = previewFile.value;
  const repo = file ? parseRepoPath(file) : null;
  if (repo) {
    const sel = selectedLines.value;
    return {
      repo_file_context: {
        repo_id: repo.repoId,
        path: repo.path,
        lines: sel ? [sel.start, sel.end] : undefined,
      },
    };
  }

  if (file) {
    return { file_context: { path: file } };
  }

  return null;
}

/**
 * Send a chat message. Side effects (modals, refreshes) are handled by
 * thread-sync.ts via ThreadEvent SSE events — no listener registry needed.
 */
export async function sendMessage(
  message: string,
  imageHashes?: string[],
  options?: { useClaudeCode?: boolean },
): Promise<void> {
  threadsLoaded.value = true;
  const eventId = crypto.randomUUID();
  const isNewThread = focusedThreadId.value === null;
  const threadId = focusedThreadId.value || eventId;

  // Check connection — show error in thread context, not just a toast
  if (!isConnected.value) {
    const map = threadMap.value;
    if (!map.has(threadId)) {
      map.set(threadId, makeOptimisticThreadState({
        id: threadId,
        title: message.slice(0, 40),
        channel: 'chat',
        initiator: 'user',
        eventsLoaded: true,
      }));
    }
    // failedSeq must be > messageSeq so ResponseFailed groups under the user's exchange.
    const messageSeq = -Date.now() - 1;
    const failedSeq = messageSeq + 1;
    const now = new Date().toISOString();
    handleEvent(map, threadId, messageSeq, {
      type: 'MessageReceived',
      text: message,
    } as StoredEvent, now);
    handleEvent(map, threadId, failedSeq, {
      type: 'ResponseFailed',
      error: getDisconnectedMsg(),
    } as StoredEvent, now);
    setFocusedThread(threadId);
    if (isNewThread) pushThreadNavState({ type: 'thread', id: threadId });
    threadMap.value = new Map(map);
    return;
  }

  setFocusedThread(threadId);
  if (isNewThread) pushThreadNavState({ type: 'thread', id: threadId });

  // Snapshot the thread BEFORE the optimistic insert below — the insert
  // creates a state='active' thread for raw new threads, which would
  // otherwise be indistinguishable from an active follow-up further down.
  const threadBeforeSend = threadMap.value.get(threadId);

  const map = threadMap.value;
  if (!map.has(threadId)) {
    map.set(threadId, makeOptimisticThreadState({
      id: threadId,
      title: message.slice(0, 40),
      channel: options?.useClaudeCode ? 'claude_code' : 'chat',
      initiator: 'user',
      eventsLoaded: true,
    }));
  }
  addPendingMessage(threadId, message, eventId, imageHashes);

  const ctx = getActiveContext();
  const body: ChatRequestBody = {
    message,
    mode: 'human',
    model: currentModel.value,
    device_id: getDeviceId(),
    reasoning_effort: reasoningEffort.value,
    event_id: eventId,
    thread_id: threadId,
  };
  if (ctx?.app_context) body.app_context = ctx.app_context;
  if (ctx?.file_context) body.file_context = ctx.file_context;
  if (ctx?.repo_file_context) body.repo_file_context = ctx.repo_file_context;
  if (imageHashes?.length) body.image_hashes = imageHashes;

  // Drafts (state='composing') ARE threads in threadMap with focusedThreadId
  // set, so neither `threadMap.get` truthiness nor `focusedThreadId === null`
  // discriminates draft from established follow-up. Use the lifecycle marker
  // against the pre-insert snapshot.
  const isUnsent = !threadBeforeSend || threadBeforeSend.meta.state === 'composing';
  const isCcThread = isUnsent
    ? !!options?.useClaudeCode
    : threadBeforeSend!.meta.channel === 'claude_code';
  if (isCcThread) {
    body.use_claude_code = true;
    if (!isUnsent && threadBeforeSend?.meta.repoId) {
      body.repo_id = threadBeforeSend.meta.repoId;
    } else if (isUnsent && selectedRepoId.value) {
      body.repo_id = selectedRepoId.value;
    }
    // Apply pending CC preferences (set from compose view before session start).
    // Don't clear pending here — they stay visible in the UI until
    // loadCommands() confirms the session adopted them (has_active_session: true).
    // Clearing early causes a race: loadCommands() fires before the session
    // exists, gets stale cache values, and with pending gone the UI shows
    // the previous session's effort/model instead of the user's selection.
    if (ccPendingModel.value !== null) {
      body.cc_model = ccPendingModel.value;
    }
    if (ccPendingReasoningEffort.value !== null) {
      body.reasoning_effort = ccPendingReasoningEffort.value;
    }
  }

  // CC ignores url_context; only send for non-CC threads. Content extraction is
  // tauri-only (browser can't read cross-origin iframes); fall back to URL+title.
  if (panelUrl.value && !body.use_claude_code) {
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
    removePendingMessage(threadId, eventId);
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
      await cancelClaudeCode(undefined, tid);
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
