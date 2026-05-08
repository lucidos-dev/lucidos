import { API_BASE, postMcpConsent } from '../../api/client';
import type { Change } from '../../api/client';
import { threadMap, focusedThreadId, changes, appliedChanges, changesHasMore, updateAvailable, applyingChangeIds, applyingNowThreadIds, generatedTitleIds, ccSessionVersion, setFocusedThread } from '../store';
import { memoryRebuildProgress, backupProgress, backupListVersion, recoveryProgress, showConfirm, showToast, repoSource } from '../store';
import { handleEvent, isChannelDefiningEvent, makeOptimisticThreadState, modeToInitiator, PENDING_TITLE_PLACEHOLDER, type ActorMode, type ThreadAggregate, type ThreadMeta, type ThreadState, type ThreadEvent, type TransientEvent } from '../thread-events';
import type { ThreadChannel } from '../store';
import type { MenuItem } from '../types';
import { handleNotificationSSE } from './notifications';
import { addRestartGroup, appliedToastRefreshAction } from './chat-changes';
import { loadPreferences } from './preferences';
import { loadArtifacts, openFilePreview, openUrl, normalizeDataPath } from './artifacts';
import { navigateToTrigger } from './triggers';
import { refreshAppUI, captureAppUI, openAppById } from './apps';
import { openCredentialRequest } from './credentials';
import { setActiveMenu, switchMenuItem, openSettingsSubview, landOnAccountsWithOverlay } from './menu';
import { pushNavState } from './navigation';
import { initPushSubscription } from './push';
import { getDeviceId, toggleDevicePush } from './devices';
import { scrollToBottom } from '../../components/chat/scrollState';
import { focusThread } from './threads';
import { refreshRepoView } from './repositories';
import { processSSEForReferences } from './entityReferences';
import { loadAllThreads, refreshThreadEvents } from './thread-loading';
import { applyRemoteCompose, pendingComposePuts } from './compose';
import { clearDraft, setDraft } from '../composeDrafts';
import { removeThreadNavEntries } from './thread-navigation';
import { isComposeFocusedHere } from '../../components/chat/promptFocus';
import { formatBytes } from '../../utils/formatBytes';

let eventSource: EventSource | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let repoChangesDebounce: ReturnType<typeof setTimeout> | null = null;

/** Set when `onerror` fires; consumed by the next `onopen` so we resync only
 *  on RECONNECT, not on the initial connect (where loadAllThreads() is already
 *  driving state via useStartup.ts). */
let needsResyncOnOpen = false;

/** In-flight resync coalescer — multiple Lagged events or back-to-back
 *  reconnects collapse into one network round-trip. */
let resyncInFlight: Promise<void> | null = null;

// Events that clear optimistic Apply Now state — the apply completed, failed,
// backend took over (merge conflict), or CC resumed work (not actually ending).
const APPLY_NOW_CLEAR_EVENTS = new Set([
  'ChangeApplied', 'ChangeApplyFailed',
  'MergeConflictDetected', 'CodingAgentToolCalled', 'CodingAgentTextStreamed',
  'CodingAgentUserMessageSent', 'CodingAgentPromptSent', 'MessageReceived',
]);

// ---------------------------------------------------------------------------
// Apply Now — deferred SessionEnded cleanup
// During Apply Now the backend kills CC (SessionEnded) then proposes changes
// (ChangeProposed). We must keep applyingNowThreadIds set during this gap.
// Safety timer: if ChangeProposed doesn't arrive within 30s, clear the state.
// ---------------------------------------------------------------------------
const applySessionEndedTimers = new Map<string, ReturnType<typeof setTimeout>>();

function startApplySessionEndedTimer(threadId: string): void {
  cancelApplySessionEndedTimer(threadId);
  applySessionEndedTimers.set(threadId, setTimeout(() => {
    applySessionEndedTimers.delete(threadId);
    if (applyingNowThreadIds.value.has(threadId)) {
      const next = new Map(applyingNowThreadIds.value);
      next.delete(threadId);
      applyingNowThreadIds.value = next;
    }
  }, 30_000));
}

function cancelApplySessionEndedTimer(threadId: string): void {
  const timer = applySessionEndedTimers.get(threadId);
  if (timer) {
    clearTimeout(timer);
    applySessionEndedTimers.delete(threadId);
  }
}

// ---------------------------------------------------------------------------
// Batched threadMap updates — coalesce rapid SSE events into one signal write
// per animation frame. Without this, every SSE event triggers O(N*M)
// recomputation in computed signals (attentionThreadCount iterates ALL threads
// and ALL events). WKWebView's tighter CPU/memory limits can crash under load.
// ---------------------------------------------------------------------------
/** Generation counter to prevent stale EventSource handlers from interfering
 *  with newer connections. Incremented on each connectThreadEvents() call.
 *  onerror/onmessage handlers check their captured generation against the
 *  current value and bail if stale (old connection's handler firing after
 *  disconnectThreadEvents() + connectThreadEvents() replaced it). */
let sseGeneration = 0;

let flushRafId: number | null = null;

/** Schedule a threadMap signal flush on the next animation frame.
 *  Multiple calls within the same frame coalesce into one flush. */
function scheduleThreadMapFlush(): void {
  if (flushRafId !== null) return;
  flushRafId = requestAnimationFrame(flushThreadMap);
}

/** Immediately flush pending threadMap changes.
 *  Creates a new Map reference to trigger Preact signal reactivity. */
export function flushThreadMap(): void {
  if (flushRafId !== null) {
    cancelAnimationFrame(flushRafId);
    flushRafId = null;
  }
  threadMap.value = new Map(threadMap.value);
}

/** Find the description for a change by looking up the matching ChangeProposed event in the thread. */
function findChangeDescription(threadId: string, changeId: string): string | undefined {
  const thread = threadMap.value.get(threadId);
  if (!thread) return undefined;
  for (const event of thread.events.values()) {
    if (event.type === 'ChangeProposed' && (event as { change_id?: string }).change_id === changeId) {
      const desc = (event as { description?: string }).description;
      if (desc) return desc.split('\n')[0];
    }
  }
  return undefined;
}

/** Build a change toast message with thread title and description/error. */
export function changeToastMessage(action: string, threadId: string, detail?: string): string {
  const thread = threadMap.value.get(threadId);
  const title = thread?.meta?.title;
  const parts: string[] = [];
  if (title) parts.push(title);
  if (detail) parts.push(detail);
  if (parts.length === 0) return `${action}.`;
  return `${action}: ${parts.join(' — ')}`;
}

export function connectThreadEvents(): void {
  if (eventSource) return;

  const gen = ++sseGeneration;
  const url = `${API_BASE}/api/events`;
  const es = new EventSource(url);
  eventSource = es;

  es.onmessage = (msg) => {
    // Stale handler — a newer connection replaced this one. The old
    // EventSource was closed, but its queued message handler can still fire.
    if (gen !== sseGeneration) return;

    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(msg.data);
    } catch {
      return;
    }

    // SSE envelope: SystemEvent uses { "type": "...", "data": {...} }, ThreadEvent uses { "type": "ThreadEvent", "data": {...} }
    const type = parsed.type as string;
    const data = (parsed.data ?? {}) as Record<string, unknown>;

    if (type === 'ThreadEvent') {
      handleThreadEvent(data);
    } else {
      handleGlobalEvent(type, data);
    }

    // Entity sync: recents, nav stack, pinned apps, store refresh.
    processSSEForReferences(type, data);
  };

  es.onopen = () => {
    if (gen !== sseGeneration) return;
    // Only resync after a reconnect — on the initial connect, useStartup.ts
    // already loads thread state. Without the flag we'd double-fetch on every
    // page load.
    if (needsResyncOnOpen) {
      needsResyncOnOpen = false;
      // Terminal BackupCompleted/BackupFailed are ephemeral: if they fired during
      // the SSE gap, the UI would stay "Backing up..." forever. Clear the signal
      // so the user can retry; an in-flight backup will repopulate on the next
      // BackupProgress event, and a duplicate POST returns 409 with a clear toast.
      backupProgress.value = null;
      resyncLoadedThreads();
    }
  };

  es.onerror = () => {
    // Stale handler — disconnectThreadEvents() already closed this
    // EventSource and connectThreadEvents() created a replacement.
    // Without this guard, the old handler closes the NEW connection
    // (via the module-scoped `eventSource` variable), causing a 3s
    // SSE gap on every iOS Safari PWA resume.
    if (gen !== sseGeneration) return;

    // Mark for resync — events emitted during the gap won't reach this tab,
    // so the next successful connect must refetch persisted state.
    needsResyncOnOpen = true;

    es.close();
    eventSource = null;
    if (reconnectTimer) clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connectThreadEvents();
    }, 3000);
  };
}

/** Refetch thread metadata + missed events for every loaded thread.
 *  Called when SSE drops + reconnects, or when the backend signals `Lagged`
 *  (its broadcast subscriber fell behind the buffer and dropped events).
 *  Without this, a tab that misses `ResponseGenerated` shows the "Thinking"
 *  spinner indefinitely while the backend has long since gone idle. */
export function resyncLoadedThreads(): Promise<void> {
  if (resyncInFlight) return resyncInFlight;
  resyncInFlight = (async () => {
    try {
      // Refresh thread-level metadata (status, section, message_count) first
      // so any per-thread refresh sees the authoritative state.
      await loadAllThreads();
      const ids = [...threadMap.value.values()]
        .filter((t) => t.eventsLoaded)
        .map((t) => t.meta.id);
      await Promise.all(ids.map((id) => refreshThreadEvents(id)));
    } finally {
      resyncInFlight = null;
    }
  })();
  return resyncInFlight;
}

export function disconnectThreadEvents(): void {
  // Bump generation BEFORE closing — any onerror handler queued by the
  // close() call will see a stale generation and bail out.
  sseGeneration++;
  // Explicit disconnect means the caller (runResumeSync, unmount) is taking
  // ownership of state recovery — don't let the next onopen also resync, or
  // every real reconnect doubles the per-thread refreshThreadEvents fan-out.
  needsResyncOnOpen = false;
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  for (const timer of applySessionEndedTimers.values()) clearTimeout(timer);
  applySessionEndedTimers.clear();
  eventSource?.close();
  eventSource = null;
}

/** Route an SSE ThreadEvent to the correct thread in threadMap.
 *  Exported for testing — not part of the public API. */
export function handleThreadEvent(data: Record<string, unknown>): void {
  const threadId = data.thread_id as string;
  const seq = typeof data.seq === 'number' ? data.seq : null;
  const event = data.event as ThreadEvent | TransientEvent;
  const created = typeof data.created === 'string' ? data.created : undefined;
  const eventId = typeof data.event_id === 'string' ? data.event_id : undefined;
  // Backend-computed projection snapshot. Present on every persisted thread
  // event, absent on transient events. handleEvent overlays it onto thread.meta.
  const aggregate = (data.aggregate && typeof data.aggregate === 'object')
    ? (data.aggregate as ThreadAggregate)
    : undefined;
  if (!threadId || !event || typeof event !== 'object' || !('type' in event)) return;
  // Live persisted events MUST carry an aggregate — its absence here (vs.
  // historical-replay where applyEventRows applies one snapshot at the end)
  // means the backend missed populating EmittedEvent.aggregate.
  if (seq !== null && !aggregate) {
    console.warn(`[SSE] persisted event ${event.type} (seq=${seq}) missing aggregate — backend bug`);
  }

  const map = threadMap.value;

  // Auto-create skeleton thread if not in map.
  // SSE-born threads set eventsLoaded=false so that loadThreadEvents will
  // backfill any events emitted before the SSE connection was established
  // (e.g. recovery threads whose MessageReceived was missed). Dedup via
  // sequence numbers ensures no duplicates when DB events overlap with SSE.
  if (!map.has(threadId)) {
    // Transient events (no seq) have no DB row — creating a skeleton would
    // produce a phantom "empty thread" that vanishes on reload. Only persisted
    // events (with seq) justify creating a new thread entry. Side effects
    // (e.g. CodingAgentThreadSpawned creating a child thread) still run.
    if (seq === null) {
      handleTransientSideEffects(event);
      return;
    }
    // Infer source from event type — coding-agent events mean claude_code, not chat
    const isCcEvent = event.type === 'SessionStarted'
      || event.type === 'SessionRecovered'
      || event.type.startsWith('CodingAgent');
    const isTriggerEvent = event.type === 'TriggerStarted';
    const isThreadStarted = event.type === 'ThreadStarted';
    const eventFields = event as Record<string, unknown>;
    const senderIsSystem = modeToInitiator(eventFields.mode as ActorMode | undefined) === 'system';
    const startedMode = isThreadStarted ? (eventFields.mode as string | undefined) : undefined;
    map.set(threadId, makeOptimisticThreadState({
      id: threadId,
      title: PENDING_TITLE_PLACEHOLDER,
      channel: (isCcEvent || startedMode === 'claude_code'
        ? 'claude_code'
        : isTriggerEvent
          ? 'trigger'
          : 'chat') as ThreadMeta['channel'],
      initiator: isTriggerEvent || senderIsSystem ? 'system' : 'user',
      eventsLoaded: isThreadStarted, // composing has no events to load
      timestamp: created,
      triggerId: isTriggerEvent ? (eventFields.trigger_id as string | undefined) : undefined,
      triggerName: isTriggerEvent ? (eventFields.trigger_name as string | undefined) : undefined,
      ...(isThreadStarted ? {
        state: 'composing' as const,
        status: 'idle' as const,
      } : {}),
    }));
    if (isThreadStarted) {
      // Seed the draft entry with the user's mode pick from ThreadStarted's
      // payload — ThreadComposeChanged will follow with text/images shortly.
      setDraft(threadId, {
        text: '',
        image_hashes: [],
        mode: startedMode === 'claude_code' ? 'claude_code' : 'lucidos',
      });
    }
  }

  // Thread is guaranteed to exist after the skeleton block above.
  const thread = map.get(threadId)!;

  // Update thread meta from lifecycle events
  if ((event.type === 'ThreadTitleGenerated' || event.type === 'ThreadTitleRenamed') && 'title' in event) {
    thread.meta.title = event.title;
    generatedTitleIds.add(threadId);
  }
  if (isChannelDefiningEvent(event.type) && 'channel' in event && event.channel) {
    thread.meta.channel = event.channel as ThreadChannel;
  }
  if (event.type === 'ChildrenCountChanged') {
    thread.meta.activeChildrenCount = event.active;
    thread.meta.totalChildrenCount = event.total;
  }
  if (event.type === 'TriggerStarted') {
    if (!thread.meta.triggerId) thread.meta.triggerId = event.trigger_id;
    if (!thread.meta.triggerName && event.trigger_name) thread.meta.triggerName = event.trigger_name;
  }

  // If a prior loadThreadEvents failed but SSE is now delivering persisted
  // events, clear the failure flag so the UI recovers from error → content.
  if (thread.eventsLoadFailed && seq != null) {
    thread.eventsLoadFailed = false;
  }

  // seq from SSE: present (number > 0) for persisted events, absent for transient
  const effectiveSeq = seq ?? null;

  handleEvent(map, threadId, effectiveSeq, event, created, eventId, aggregate);
  scheduleThreadMapFlush();

  // Compose-clear must yield to a user actively typing here. Two layers:
  //   1. Origin-device echo: when the send/discard came from this device,
  //      sendCompose/discardCompose already mutated local compose state
  //      synchronously. The SSE echo arrives later and would blank any text
  //      the user has started typing for the next message — drop it.
  //   2. Focused textarea (cross-device fallback): when a peer device sends
  //      and this user is mid-keystroke, dropping the inbound clear preserves
  //      the local PUT that may not have round-tripped yet.
  if (event.type === 'MessageReceived') {
    thread.meta.state = 'active';
    if (!isFromThisDevice(event)) clearComposeIfUnfocused(thread, threadId);
  }
  if (event.type === 'ThreadDiscarded') {
    thread.meta.state = 'discarded';
    if (!isFromThisDevice(event)) {
      // Without releasing focus + nav, ThreadPane keeps routing to ThreadView
      // (state ≠ 'composing') and shows the empty-state instead of the fresh
      // compose layout. Skipped while typing here so keystrokes aren't yanked.
      if (!isComposeFocusedHere(threadId)) {
        if (focusedThreadId.value === threadId) setFocusedThread(null);
        removeThreadNavEntries(threadId);
      }
      clearComposeIfUnfocused(thread, threadId);
    }
  }
  if (event.type === 'ThreadArchived') {
    thread.meta.state = 'archived';
  }

  // Bump CC session version so CCControlMenu re-fetches commands.
  // CodingAgentUserMessageSent covers follow-ups to idle CC sessions
  // (no SessionStarted fires for those — the existing process resumes).
  // CodingAgentIdled guarantees CC binary is initialized — retry from
  // SessionStarted may have exhausted before Init arrived.
  if (event.type === 'SessionStarted' || event.type === 'SessionRecovered'
      || event.type === 'SessionEnded' || event.type === 'CodingAgentUserMessageSent'
      || event.type === 'CodingAgentIdled' || event.type === 'CodingAgentSettingsChanged') {
    ccSessionVersion.value++;
  }

  // No auto-read on focus — user must explicitly click Archive, Apply, or Discard.

  // Dispatch side effects for transient events
  handleTransientSideEffects(event);

  // Manage optimistic "Apply Now" phase transitions.
  // 'requesting' → 'applying' on ChangeProposed (backend started the merge).
  // Clear entirely on events that mean the apply completed, failed, or backend took over.
  //
  // SessionEnded is special: during Apply Now the backend kills CC first (SessionEnded)
  // then proposes the change (ChangeProposed). So SessionEnded must NOT clear the phase
  // immediately — instead we defer with a safety timeout, cancelled if ChangeProposed
  // or any other resolution event arrives.
  if (applyingNowThreadIds.value.has(threadId)) {
    if (event.type === 'ChangeProposed') {
      cancelApplySessionEndedTimer(threadId);
      const next = new Map(applyingNowThreadIds.value);
      next.set(threadId, 'applying');
      applyingNowThreadIds.value = next;
      // Mark the change as applying in the Changes panel too — prevents brief
      // "Apply"/"Discard" buttons on the newly-proposed change during Apply Now.
      const changeId = (event as { change_id?: string }).change_id;
      if (changeId) {
        applyingChangeIds.value = new Set([...applyingChangeIds.value, changeId]);
      }
    } else if (event.type === 'SessionEnded') {
      startApplySessionEndedTimer(threadId);
    } else if (APPLY_NOW_CLEAR_EVENTS.has(event.type)) {
      cancelApplySessionEndedTimer(threadId);
      const next = new Map(applyingNowThreadIds.value);
      next.delete(threadId);
      applyingNowThreadIds.value = next;
    }
  }

  // Toast for change state transitions
  if (event.type === 'ChangeApplied') {
    const changeId = (event as { change_id?: string }).change_id;
    const desc = changeId ? findChangeDescription(threadId, changeId) : undefined;
    const requiresRestart = !!(event as { requires_restart?: boolean }).requires_restart;
    const clientUpdate = !!(event as { client_update?: boolean }).client_update;
    const applyKey = `applying-${threadId}`;
    const refreshAction = appliedToastRefreshAction(requiresRestart, clientUpdate);
    showToast(changeToastMessage('Applied', threadId, desc), 'success', {
      key: applyKey,
      onClick: () => focusThread(threadId),
      action: refreshAction,
      autoDismissMs: 4000,
    });
    // Set restart toast immediately from the thread event — don't wait for
    // the separate ChangesUpdated system event. If ChangesUpdated is missed
    // (SSE drop, Vite reload race), this ensures the toast appears.
    if (requiresRestart) {
      const commits = (event as { commits?: string[] }).commits ?? [];
      const eventTitle = (event as { thread_title?: string }).thread_title;
      const threadTitle = eventTitle ?? threadMap.value.get(threadId)?.meta.title ?? 'Untitled thread';
      addRestartGroup({ threadId, threadTitle, commits });
    }
    if (clientUpdate) {
      updateAvailable.value = true;
    }
  } else if (event.type === 'ChangeDiscarded') {
    const changeId = (event as { change_id?: string }).change_id;
    const desc = changeId ? findChangeDescription(threadId, changeId) : undefined;
    showToast(changeToastMessage('Discarded', threadId, desc), 'success', {
      key: `discarding-${threadId}`,
      onClick: () => focusThread(threadId),
      autoDismissMs: 4000,
    });
  } else if (event.type === 'ChangeReverted') {
    const changeId = (event as { change_id?: string }).change_id;
    const desc = changeId ? findChangeDescription(threadId, changeId) : undefined;
    showToast(changeToastMessage('Reverted', threadId, desc), 'success');
  } else if (event.type === 'ChangeApplyFailed') {
    const error = (event as { error?: string }).error ?? 'Unknown error';
    showToast(changeToastMessage('Failed to apply', threadId, error), 'error', { key: `applying-${threadId}`, onClick: () => focusThread(threadId) });
  }

  // After apply/discard/revert, scroll to bottom and reveal the app header
  // on mobile so the user sees the result with full navigation visible.
  if (event.type === 'ChangeApplied' || event.type === 'ChangeDiscarded' || event.type === 'ChangeReverted') {
    if (threadId === focusedThreadId.value) {
      scrollToBottom();
      document.dispatchEvent(new Event('reveal-mobile-header'));
    }
  }

  // Clear applyingChangeIds when a change is resolved.
  if (event.type === 'ChangeApplied' || event.type === 'ChangeApplyFailed') {
    const changeId = (event as { change_id?: string }).change_id;
    if (changeId && applyingChangeIds.value.has(changeId)) {
      const next = new Set(applyingChangeIds.value);
      next.delete(changeId);
      applyingChangeIds.value = next;
    }
  }

  // Track change_id as "applying" when merge conflict resolution starts.
  if (event.type === 'MergeConflictDetected') {
    const changeId = (event as { change_id?: string }).change_id;
    if (changeId && !applyingChangeIds.value.has(changeId)) {
      applyingChangeIds.value = new Set([...applyingChangeIds.value, changeId]);
    }
  }
}

export function handleGlobalEvent(type: string, data: Record<string, unknown>): void {
  switch (type) {
    case 'NotificationCreated':
    case 'NotificationRead':
    case 'NotificationsAllRead':
      handleNotificationSSE();
      break;

    case 'PreferencesChanged':
      loadPreferences();
      break;

    case 'MemoryRebuildProgress': {
      const processed = (data.processed as number) ?? 0;
      const total = (data.total as number) ?? 0;
      const percent = (data.percent as number) ?? 0;
      memoryRebuildProgress.value = { processed, total, percent };
      if (processed >= total && total > 0) {
        setTimeout(() => { memoryRebuildProgress.value = null; }, 2000);
      }
      break;
    }

    case 'ChangesUpdated': {
      const pending = (data.pending ?? []) as Change[];
      const applied = (data.applied ?? []) as Change[];
      const totalPending = (data.total_pending as number) ?? 0;
      changes.value = pending;
      appliedChanges.value = applied;
      changesHasMore.value = totalPending > pending.length;
      // restartRequired is intentionally not touched here — stale SSE values
      // would otherwise dismiss an active restart toast.
      // Debounce repo-scoped refresh — ChangesUpdated fires on every change globally
      if (repoChangesDebounce) clearTimeout(repoChangesDebounce);
      repoChangesDebounce = setTimeout(() => {
        const currentRepo = repoSource.value;
        if (currentRepo) refreshRepoView(currentRepo);
      }, 300);
      break;
    }

    case 'BackupProgress': {
      const phase = (data.phase as string) ?? '';
      const progress = (data.progress as number) ?? 0;
      const total = (data.total as number) ?? 0;
      backupProgress.value = { phase, progress, total };
      // BackupCompleted/BackupFailed clear progress; auto-clearing on 100%
      // would null a follow-up backup started <2s later.
      break;
    }

    case 'BackupCompleted': {
      backupProgress.value = null;
      const filename = String(data.filename ?? '');
      const size = formatBytes(Number(data.size_bytes ?? 0));
      showToast(`Backup created: ${filename} (${size})`, 'success');
      backupListVersion.value++;
      break;
    }

    case 'BackupFailed': {
      backupProgress.value = null;
      showToast(`Backup failed: ${String(data.error ?? 'Unknown error')}`, 'error');
      break;
    }

    case 'RecoveryProgress': {
      const completed = (data.completed as number) ?? 0;
      const total = (data.total as number) ?? 0;
      recoveryProgress.value = { completed, total };
      if (completed >= total && total > 0) {
        setTimeout(() => { recoveryProgress.value = null; }, 3000);
      }
      break;
    }

    case 'Toast': {
      const message = (data.message as string) ?? '';
      const level = (data.level as string) ?? 'info';
      if (message) showToast(message, level as 'success' | 'info' | 'error' | 'warning');
      break;
    }

    case 'Lagged': {
      // Backend signals our broadcast subscriber fell behind the buffer and
      // dropped events. Refetch state so any "in-flight" UI (Thinking spinner,
      // streaming exchange) reconciles with the now-completed backend state.
      const count = (data.count as number) ?? 0;
      console.warn(`[SSE] Stream lagged by ${count} events — resyncing loaded threads`);
      resyncLoadedThreads();
      break;
    }

    // ThreadComposeChanged is the SSE-only ephemeral notification emitted on
    // every compose PUT. Routed to compose.ts which writes the threadMap
    // entry's compose fields. Three guards layered together:
    //   1. origin_device_id — server rebroadcasts to all devices including
    //      the originator; ignore our own echo.
    //   2. pendingComposePuts — a debounced PUT may already be in flight
    //      with newer text; the SSE event for our previous PUT could clobber
    //      it on arrival otherwise.
    //   3. focused-textarea — if the user is mid-keystroke on this thread's
    //      input, dropping the inbound update is safer than blanking what
    //      they just typed.
    case 'ThreadComposeChanged': {
      const originDeviceId = data.origin_device_id as string | undefined;
      if (originDeviceId && originDeviceId === getDeviceId()) break;
      const id = data.id as string;
      if (pendingComposePuts.has(id)) break;
      if (isComposeFocusedHere(id)) break;
      const modeRaw = data.mode as string | undefined;
      applyRemoteCompose(id, {
        text: (data.text as string) ?? '',
        image_hashes: Array.isArray(data.image_hashes) ? data.image_hashes as string[] : [],
        mode: modeRaw === 'claude_code' ? 'claude_code' : modeRaw === 'lucidos' ? 'lucidos' : null,
      });
      break;
    }
  }
}

/** True when an inbound thread event was emitted by this browser. Per-event
 *  field: `MessageReceived.device_id` (HTTP-boundary legacy field) or
 *  `ThreadDiscarded.actor.device_id` (structured MessageOrigin). Matching
 *  means the local mutating action already updated state synchronously —
 *  applying the SSE echo would clobber any keystrokes the user has typed
 *  since the local mutation. */
function isFromThisDevice(event: ThreadEvent | TransientEvent): boolean {
  const me = getDeviceId();
  switch (event.type) {
    case 'MessageReceived':
      return event.device_id === me;
    case 'ThreadDiscarded':
      return event.actor?.kind === 'device' && event.actor.device_id === me;
    default:
      return false;
  }
}

function clearComposeIfUnfocused(_thread: ThreadState, threadId: string): void {
  if (isComposeFocusedHere(threadId)) return;
  clearDraft(threadId);
}

/** Handle transient ThreadEvent types that trigger side effects (modals, refreshes). */
function handleTransientSideEffects(event: ThreadEvent | TransientEvent): void {
  switch (event.type) {
    case 'CredentialRequest':
      try {
        openCredentialRequest(JSON.parse((event as { payload: string }).payload));
      } catch (e) {
        console.error('Failed to parse credential request:', e);
        showToast('Failed to handle credential request from engine', 'error');
      }
      break;

    case 'EmailConfirmRequest':
      try {
        const request = JSON.parse((event as { payload: string }).payload);
        landOnAccountsWithOverlay({ type: 'form', form: { type: 'email-confirm', request } });
        pushNavState();
      } catch (e) {
        console.error('Failed to parse email confirm request:', e);
        showToast('Failed to handle email confirm request from engine', 'error');
      }
      break;

    case 'PushNotificationRequest':
      (async () => {
        const ok = await showConfirm(
          'Enable push notifications?',
          'Enable',
          { variant: 'default' }
        );
        if (ok) {
          await initPushSubscription();
          await toggleDevicePush(getDeviceId(), true);
        }
      })();
      break;

    case 'McpConsentRequest':
      (async () => {
        try {
          const { request_id, server_name, tool_name, arguments_summary } = JSON.parse((event as { data: string }).data);
          const msg = `**${server_name}** wants to call **${tool_name}**\n\n\`\`\`json\n${arguments_summary}\n\`\`\``;
          const ok = await showConfirm(msg, 'Allow', { variant: 'default' });
          await postMcpConsent(request_id, ok);
        } catch (e) {
          console.error('[SSE] Failed to handle MCP consent request:', e);
          showToast('Failed to send MCP consent response', 'error');
        }
      })();
      break;

    case 'RefreshFile':
      loadArtifacts();
      break;

    case 'ToolResult': {
      const name = (event as { name: string }).name;
      if (['write_file', 'edit_file', 'copy_file', 'delete_file', 'import_file'].includes(name)) {
        loadArtifacts();
      }
      break;
    }

    case 'RefreshAppUI':
      refreshAppUI((event as { app_id: string }).app_id);
      break;

    case 'CaptureAppUI': {
      const e = event as { app_id: string; request_id: string };
      captureAppUI(e.app_id, e.request_id);
      break;
    }

    case 'NavigationRequested':
      try {
        const nav = JSON.parse((event as { payload: string }).payload);
        handleNavigationRequest(nav);
      } catch (e) {
        console.error('[SSE] Failed to parse navigation request:', e);
        showToast('Failed to handle navigation request from engine', 'error');
      }
      break;

    case 'CodingAgentThreadSpawned': {
      const e = event as { cc_thread_id: string; title: string };
      const map = threadMap.value;

      // Move pendingUserMessages from the current focused thread to the CC thread.
      // The user typed in the original thread, but the message should appear in the CC thread.
      const currentThread = focusedThreadId.value ? map.get(focusedThreadId.value) : null;
      const userMessages = currentThread?.pendingUserMessages.length ? [...currentThread.pendingUserMessages] : [];
      if (currentThread) {
        currentThread.pendingUserMessages = [];
      }

      // Create the CC thread with proper title and source
      // Inherit initiator from parent — if parent is system-initiated, CC sub-thread is too
      if (!map.has(e.cc_thread_id)) {
        map.set(e.cc_thread_id, makeOptimisticThreadState({
          id: e.cc_thread_id,
          title: e.title,
          channel: 'claude_code',
          initiator: currentThread?.meta.initiator ?? 'user',
          eventsLoaded: false,
          pendingUserMessages: userMessages,
        }));
      }
      flushThreadMap();  // Immediate — user needs to see the new thread now
      break;
    }
  }
}

/** Handle a NavigationRequested event — dispatches to the correct UI action based on target. */
export function handleNavigationRequest(nav: {
  target: string;
  settings_view?: string;
  app_id?: string;
  file_path?: string;
  url?: string;
  id?: string;
}): void {
  const navAppId = nav.app_id;
  switch (nav.target) {
    case 'files':
    case 'apps':
    case 'triggers':
    case 'changes':
    case 'notifications':
      switchMenuItem(nav.target as MenuItem);
      break;
    case 'settings':
      switchMenuItem('settings');
      if (nav.settings_view) {
        openSettingsSubview(nav.settings_view as 'devices' | 'accounts' | 'backup' | 'memory' | 'repositories');
      }
      break;
    case 'app':
      if (navAppId) openAppById(navAppId);
      break;
    case 'app-ui':
      if (navAppId) openAppById(navAppId);
      break;
    case 'file':
      if (nav.file_path) openFilePreview(normalizeDataPath(nav.file_path));
      break;
    case 'trigger':
      if (nav.id) navigateToTrigger(nav.id);
      break;
    case 'thread':
      if (nav.id) focusThread(nav.id);
      break;
    case 'new-app':
      // Single nav push — switchMenuItem would push (apps, no overlay) first,
      // stranding Back on an empty Apps list.
      setActiveMenu('apps', { type: 'form', form: { type: 'new-app' } });
      pushNavState();
      break;
    case 'new-trigger':
      setActiveMenu('triggers', { type: 'form', form: { type: 'trigger' } });
      pushNavState();
      break;
    case 'url':
      if (nav.url) openUrl(nav.url);
      break;
  }
}
