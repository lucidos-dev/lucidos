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
} from '../store';
import { promoteDraftToThread } from './drafts';
import { toFailed } from '../types';
import type { ChatRequestBody } from '../../api/types';
import { API_BASE, submitChat, cancelChat, cancelClaudeCode, interruptClaudeCode } from '../../api/client';
import { getDisconnectedMsg } from './connection';
import { getDeviceId } from './devices';
import { handleEvent, type ThreadState, type StoredEvent } from '../thread-events';
import { scrollToBottom } from '../../components/chat/scrollState';
import { refreshThreadEvents } from './thread-loading';
import { isTauri } from '../../utils/platform';
import { getWebviewContent } from '../../utils/tauri';
import { FOCUSED_THREAD_KEY } from '../../utils/draftStorage';
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
  images?: Array<{ base64: string; mimeType: string }>,
): void {
  const map = threadMap.value;
  const thread = map.get(threadId);
  if (thread) {
    thread.pendingUserMessages.push({
      text: message,
      eventId,
      created: new Date().toISOString(),
      images: images?.map(img => ({ base64: img.base64, mime_type: img.mimeType })),
    });
    scrollToBottom();
    threadMap.value = new Map(map);
  }
}

/** Load registered repositories from the backend. */
export async function loadRepositories(): Promise<void> {
  repositories.value = { status: 'loading' };
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
 *
 * `composeDraftId` is set when sending from a fresh compose draft so the
 * caller's unsent draft entry can be cleaned up once the new thread takes
 * over its identity.
 */
export async function sendMessage(
  message: string,
  images?: Array<{ base64: string; mimeType: string }>,
  options?: { useClaudeCode?: boolean; composeDraftId?: string },
): Promise<void> {
  threadsLoaded.value = true;
  const eventId = crypto.randomUUID();
  const threadId = focusedThreadId.value || eventId;

  // Check connection — show error in thread context, not just a toast
  if (!isConnected.value) {
    const map = threadMap.value;
    if (!map.has(threadId)) {
      map.set(threadId, {
        meta: { id: threadId, title: message.slice(0, 40), channel: 'chat', initiator: 'user', pinned: false, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), unread: false, status: 'running', messageCount: 0, section: 'default', activeChildrenCount: 0, totalChildrenCount: 0, ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: new Date().toISOString() },
        events: new Map(),
        streamingBuffer: '',
        eventsLoaded: true,
        eventsLoadFailed: false,
        lastDbSeq: 0,
        pendingUserMessages: [],
      });
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
    focusedThreadId.value = threadId;
    localStorage.setItem(FOCUSED_THREAD_KEY, threadId);
    threadMap.value = new Map(map);
    return;
  }

  // Update event-driven store
  focusedThreadId.value = threadId;
  localStorage.setItem(FOCUSED_THREAD_KEY, threadId);

  // Clean up the originating compose draft so the next Compose starts blank.
  if (options?.composeDraftId && options.composeDraftId !== threadId) {
    promoteDraftToThread(options.composeDraftId);
  }

  // Set optimistic pending message
  const map = threadMap.value;
  if (!map.has(threadId)) {
    const newThread: ThreadState = {
      meta: {
        id: threadId,
        title: message.slice(0, 40),
        channel: options?.useClaudeCode ? 'claude_code' : 'chat',
        initiator: 'user',
        pinned: false,
        createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
        unread: false,
        status: 'running',
        messageCount: 0,
        section: 'default',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        ccHasChanges: false,
        ccRequiresRestart: false,
        ccIsExternalRepo: false,
        ccApplying: false,
        lastRevivedAt: new Date().toISOString(),
      },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: true,
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    map.set(threadId, newThread);
  }
  addPendingMessage(threadId, message, eventId, images);

  // Build and submit request
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
  if (images?.length) body.images = images.map(img => ({ base64: img.base64, mime_type: img.mimeType }));

  // Inherit CC mode from existing thread or explicit option
  const existingThread = threadMap.value.get(threadId);
  if (options?.useClaudeCode || existingThread?.meta.channel === 'claude_code') {
    body.use_claude_code = true;
    if (selectedRepoId.value) {
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

  // Include URL context when a page is open in the panel (CC doesn't use it)
  if (panelUrl.value && !body.use_claude_code) {
    if (isTauri()) {
      try {
        const { title, content } = await getWebviewContent();
        body.url_context = {
          url: panelUrl.value,
          title: title || panelTitle.value || undefined,
          content: content.trim() ? content : '',
        };
      } catch {
        // Content extraction failed — still send the URL so the LLM knows what page is open
        body.url_context = {
          url: panelUrl.value,
          title: panelTitle.value || undefined,
          content: '',
        };
      }
    } else {
      // Browser mode: can't extract cross-origin iframe content, send URL only
      body.url_context = {
        url: panelUrl.value,
        title: panelTitle.value || undefined,
        content: '',
      };
    }
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
 * Cancel the current processing exchange.
 * Fires the appropriate cancel API based on thread type.
 * Returns false if the API call failed — caller resets optimistic UI.
 */
export async function cancelCurrentExchange(): Promise<boolean> {
  const threadId = focusedThreadId.value ?? undefined;
  try {
    const thread = threadId ? threadMap.value.get(threadId) : undefined;
    if (thread?.meta.channel === 'claude_code') {
      await cancelClaudeCode(undefined, threadId);
    } else {
      await cancelChat(threadId);
    }
    return true;
  } catch (err) {
    showToast(`Failed to cancel: ${errorDetail(err)}`, 'error');
    return false;
  }
}

/**
 * Interrupt the current Claude Code exchange — stops the current work
 * but keeps the session alive (like pressing Esc in Claude Code terminal).
 * Returns false if the API call failed — caller resets optimistic UI.
 */
export async function interruptCurrentExchange(): Promise<boolean> {
  const threadId = focusedThreadId.value ?? undefined;
  try {
    await interruptClaudeCode(threadId);
    return true;
  } catch (err) {
    showToast(`Failed to interrupt: ${errorDetail(err)}`, 'error');
    return false;
  }
}
