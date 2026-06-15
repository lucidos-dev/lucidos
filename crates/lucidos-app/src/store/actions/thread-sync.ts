import { API, postMcpConsent } from '../../api/client';
import type { Change } from '../../api/client';
import { threadMap, focusedThreadId, changes, appliedChanges, changesHasMore, updateAvailable, applyingChangeIds, applyingNowThreadIds, applyAllInProgress, generatedTitleIds, codingAgentSessionVersion, setFocusedThread, archivingThreadIds } from '../store';
import { memoryRebuildProgress, backupProgress, restoreState, backupListVersion, backupStatusVersion, recoveryProgress, showConfirm, showToast, repoSource, TOAST_AUTO_DISMISS_MS } from '../store';
import { handleEvent, isChannelDefiningEvent, makeOptimisticThreadState, modeToInitiator, PENDING_TITLE_PLACEHOLDER, type ActorMode, type ThreadAggregate, type ThreadMeta, type ThreadEvent, type TransientEvent } from '../thread-events';
import { bumpThreadEvents } from '../threadActivity';
import type { ThreadChannel } from '../store';
import { handleNotificationSSE } from './notifications';
import { handlePresenceCheck, type PresenceCheckPayload } from './presence-pong';
import {
  handleNotificationToastRequested,
  type NotificationToastRequestedPayload,
} from './in-app-notification-toast';
import {
  handleNativePushRequested,
  type NativePushRequestedPayload,
} from './native-push';
import { addRestartGroup, appliedToastRefreshAction } from './chat-changes';
import { scheduleServiceWorkerUpdateChecks } from '../../hooks/sw-update';
import { loadPreferences } from './preferences';
import { loadArtifacts } from './artifacts';
import { refreshAppUI, captureAppUI } from './apps';
import { clearWipIfMatches } from './wipPreview';
import { openCredentialRequest } from './credentials';
import { openPluginInstallRequest } from './plugin-install';
import { openPluginUninstallRequest } from './plugin-uninstall';
import { landOnAccountsWithOverlay } from './menu';
import { pushNavState } from './navigation';
import { initPushSubscription } from './push';
import { getDeviceId, toggleDevicePush } from './devices';
import { scrollToBottom } from '../../components/chat/scrollState';
import { focusThread } from './threads';
import { formatThreadLabel } from './thread-label';
import { refreshRepoView } from './repositories';
import { processSSEForReferences } from './entityReferences';
import { loadAllThreads, refreshThreadEvents, loadThreadEvents } from './thread-loading';
import { applyRemoteCompose, pendingComposePuts } from './compose';
import { clearDraft, setDraft } from '../composeDrafts';
import { removeThreadNavEntries } from './thread-navigation';
import { isComposeFocusedHere } from '../../components/chat/promptFocus';
import { formatBytes } from '../../utils/formatBytes';
import { errorDetail } from '../../utils/errorDetail';
import { handleNavigationRequest } from './navigation-request';

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

/** Rebuild the thread's events Map and force a full re-fetch. Called by the
 *  ThreadView watchdog when has()/get() on the long-lived Map return wrong
 *  results — iOS Safari can corrupt Map internals under memory pressure.
 *  Caller is responsible for the eligibility check (has CONTENT events but
 *  exchanges are empty) and retry capping. */
export function rebuildCorruptedThreadEvents(threadId: string): void {
  const thread = threadMap.value.get(threadId);
  if (!thread) return;
  thread.events = new Map(thread.events);
  thread.eventsLoaded = false;
  thread.lastDbSeq = 0;
  void loadThreadEvents(threadId);
}

/** Find the description for a change by looking up the matching ChangeProposed event in the thread. */
function findChangeDescription(threadId: string, changeId: string): string | undefined {
  const thread = threadMap.value.get(threadId);
  if (!thread) return undefined;
  for (const event of thread.events.values()) {
    if (event.type === 'ChangeProposed' && event.change_id === changeId) {
      if (event.description) return event.description.split('\n')[0];
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
  const url = `${API}/events`;
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
      // `resyncLoadedThreads` coalesces and surfaces per-thread failures via
      // the Loadable/showToast paths inside loadAllThreads + refreshThreadEvents.
      void resyncLoadedThreads();
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

  // Track meta-shape changes across the entire handler — used to gate the
  // global `threadMap` signal flush at the bottom. Per-thread event arrivals
  // bump `threadEventsBump` unconditionally; the wide flush only fires when
  // a meta field consumers care about (status, title, channel, child counts,
  // codingAgent flags, etc.) actually changed. Without the gate, every CC
  // streaming token would re-execute attentionThreadCount + every visible
  // ChatExchange. See `store/threadActivity.ts` and the plan in
  // ~/.claude/plans/generic-sparking-garden.md.
  let metaChanged = false;

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
      || event.type === 'ContinuationStarted'
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
    // New thread row inserted into the map — every subscriber needs to learn
    // about it (drawer rows, attentionThreadCount, focused-thread router).
    metaChanged = true;
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
    metaChanged = true;
  }
  if (isChannelDefiningEvent(event.type) && 'channel' in event && event.channel) {
    if (thread.meta.channel !== event.channel) {
      thread.meta.channel = event.channel as ThreadChannel;
      metaChanged = true;
    }
  }
  if (event.type === 'ChildrenCountChanged') {
    if (thread.meta.activeChildrenCount !== event.active || thread.meta.totalChildrenCount !== event.total) {
      thread.meta.activeChildrenCount = event.active;
      thread.meta.totalChildrenCount = event.total;
      metaChanged = true;
    }
  }
  if (event.type === 'TriggerStarted') {
    if (!thread.meta.triggerId) { thread.meta.triggerId = event.trigger_id; metaChanged = true; }
    if (!thread.meta.triggerName && event.trigger_name) { thread.meta.triggerName = event.trigger_name; metaChanged = true; }
  }

  // If a prior loadThreadEvents failed but SSE is now delivering persisted
  // events, clear the failure flag so the UI recovers from error → content.
  // ThreadView reads `eventsLoadFailed` in its render path; treat the flip as
  // a meta-shape change so the global flush wakes it up.
  if (thread.eventsLoadFailed && seq != null) {
    thread.eventsLoadFailed = false;
    metaChanged = true;
  }

  // seq from SSE: present (number > 0) for persisted events, absent for transient
  const effectiveSeq = seq ?? null;

  const handled = handleEvent(map, threadId, effectiveSeq, event, created, eventId, aggregate);
  if (handled.metaChanged) metaChanged = true;

  // Archive race guard. Every persisted SSE event carries the projection
  // snapshot AT EVENT EMIT TIME. When the backend processes a cascade
  // archive it emits CodingAgentIdled for each descendant BEFORE the
  // ThreadArchived row update lands — that intermediate aggregate still
  // has section='inbox' and coding_agent_proposed=true. Without this guard
  // applyAggregateToMeta reverts the optimistic flip, the row flies back
  // to Review until the matching ThreadArchived SSE lands ~14ms later,
  // and neighbours visibly shift twice. archivingThreadIds (set by
  // handleArchiveThread for every cascade member) is the in-flight signal;
  // it clears in handleArchiveThread's finally so post-archive SSE events
  // apply their aggregate normally.
  if (aggregate && archivingThreadIds.value.has(threadId)) {
    if (thread.meta.section !== 'archived' || thread.meta.codingAgentProposed) {
      thread.meta.section = 'archived';
      thread.meta.codingAgentProposed = false;
      metaChanged = true;
    }
  }

  // Compose-clear must yield to a user actively typing here. Two layers:
  //   1. Origin-device echo: when the send/discard came from this device,
  //      sendCompose/discardCompose already mutated local compose state
  //      synchronously. The SSE echo arrives later and would blank any text
  //      the user has started typing for the next message — drop it.
  //   2. Focused textarea (cross-device fallback): when a peer device sends
  //      and this user is mid-keystroke, dropping the inbound clear preserves
  //      the local PUT that may not have round-tripped yet.
  if (event.type === 'MessageReceived') {
    if (thread.meta.state !== 'active') { thread.meta.state = 'active'; metaChanged = true; }
    if (!isFromThisDevice(event)) clearComposeIfUnfocused(threadId);
  }
  if (event.type === 'ThreadDiscarded') {
    if (thread.meta.state !== 'discarded') { thread.meta.state = 'discarded'; metaChanged = true; }
    if (!isFromThisDevice(event)) {
      // Without releasing focus + nav, ThreadPane keeps routing to ThreadView
      // (state ≠ 'composing') and shows the empty-state instead of the fresh
      // compose layout. Skipped while typing here so keystrokes aren't yanked.
      if (!isComposeFocusedHere(threadId)) {
        if (focusedThreadId.value === threadId) setFocusedThread(null);
        removeThreadNavEntries(threadId);
      }
      clearComposeIfUnfocused(threadId);
    }
  }
  // ThreadArchived deliberately does NOT touch `meta.state`: the compose
  // state machine is orthogonal to archive routing (an archived thread
  // stays at state='active' and only flips `archive_state` / `meta.section`).
  // The section flip is handled by the "Archive race guard" block above
  // for cascade members and by `applyAggregateToMeta` for direct archives,
  // both keyed off `archive_state` not `state`.

  // Bump Claude Code session version so CodingAgentControlMenu re-fetches commands.
  // CodingAgentUserMessageSent covers follow-ups to idle Claude Code sessions
  // (no SessionStarted fires for those — the existing process resumes).
  // CodingAgentIdled guarantees CC binary is initialized — retry from
  // SessionStarted may have exhausted before Init arrived.
  if (event.type === 'SessionStarted' || event.type === 'ContinuationStarted'
      || event.type === 'SessionEnded' || event.type === 'CodingAgentUserMessageSent'
      || event.type === 'CodingAgentIdled' || event.type === 'CodingAgentSettingsChanged') {
    codingAgentSessionVersion.value++;
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
      if (event.change_id) {
        applyingChangeIds.value = new Set([...applyingChangeIds.value, event.change_id]);
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
    const desc = event.change_id ? findChangeDescription(threadId, event.change_id) : undefined;
    const requiresRestart = !!event.requires_restart;
    const clientUpdate = !!event.client_update;
    const applyKey = `applying-${threadId}`;
    const refreshAction = appliedToastRefreshAction(requiresRestart, clientUpdate);
    showToast(changeToastMessage('Applied', threadId, desc), 'success', {
      key: applyKey,
      onClick: () => focusThread(threadId),
      action: refreshAction,
      autoDismissMs: TOAST_AUTO_DISMISS_MS,
    });
    // Set restart toast immediately from the thread event — don't wait for
    // the separate ChangesUpdated system event. If ChangesUpdated is missed
    // (SSE drop, Vite reload race), this ensures the toast appears.
    if (requiresRestart) {
      const commits = event.commits ?? [];
      const threadTitle = event.thread_title ?? threadMap.value.get(threadId)?.meta.title ?? 'Untitled thread';
      addRestartGroup({ threadId, threadTitle, commits });
    }
    if (clientUpdate) {
      updateAvailable.value = true;
      // In --built mode the frontend rebuilds over the next few seconds; nudge
      // the SW to pick up the new build so the Refresh toast fires promptly.
      // No-op in the live dev server (sw.js never changes).
      scheduleServiceWorkerUpdateChecks();
    }
  } else if (event.type === 'ChangeDiscarded') {
    const desc = event.change_id ? findChangeDescription(threadId, event.change_id) : undefined;
    showToast(changeToastMessage('Discarded', threadId, desc), 'success', {
      key: `discarding-${threadId}`,
      onClick: () => focusThread(threadId),
      autoDismissMs: TOAST_AUTO_DISMISS_MS,
    });
  } else if (event.type === 'ChangeReverted') {
    const desc = event.change_id ? findChangeDescription(threadId, event.change_id) : undefined;
    showToast(changeToastMessage('Reverted', threadId, desc), 'success');
  } else if (event.type === 'ChangeApplyFailed') {
    const error = event.error ?? 'Unknown error';
    showToast(changeToastMessage('Failed to apply', threadId, error), 'error', { key: `applying-${threadId}`, onClick: () => focusThread(threadId) });
  }

  // After apply/discard/revert, scroll to bottom and reveal the app header
  // on mobile so the user sees the result with full navigation visible.
  if (event.type === 'ChangeApplied' || event.type === 'ChangeDiscarded' || event.type === 'ChangeReverted') {
    if (threadId === focusedThreadId.value) {
      scrollToBottom();
      document.dispatchEvent(new Event('reveal-mobile-header'));
    }
    // Any terminal change event for a thread removes its worktree (Apply
    // ff-merges + cleans up, Discard deletes the branch + worktree). Drop
    // the WIP preview if it was pointing at this thread — the WIP URL is
    // about to start returning 404. AppUiRefreshRequested covers the
    // Apply-with-iframe-bundled-edit subset; this covers Discard and
    // Apply-of-non-bundled edits (artifacts, knowhow under the app).
    clearWipIfMatches((wipTid) => wipTid === threadId);
  }
  if (event.type === 'ThreadArchived') {
    clearWipIfMatches((wipTid) => wipTid === threadId);
  }

  // Clear applyingChangeIds when a change is resolved.
  if (event.type === 'ChangeApplied' || event.type === 'ChangeApplyFailed') {
    if (event.change_id && applyingChangeIds.value.has(event.change_id)) {
      const next = new Set(applyingChangeIds.value);
      next.delete(event.change_id);
      applyingChangeIds.value = next;
    }
  }

  // Track change_id as "applying" when merge conflict resolution starts.
  if (event.type === 'MergeConflictDetected') {
    if (event.change_id && !applyingChangeIds.value.has(event.change_id)) {
      applyingChangeIds.value = new Set([...applyingChangeIds.value, event.change_id]);
    }
    // Event-driven toast so all three engine paths (Apply Now, Apply All,
    // Tier-2 recovery) notify uniformly. Fires regardless of focus/visibility
    // — the inline panel and the toast each pull their weight (the panel is
    // local context, the toast is a system-level "this is happening" cue).
    // Keyed by thread+change so a Tier-2 → Tier-3 cascade (both paths emit
    // MergeConflictDetected for the same change to open their own panels)
    // refreshes a single toast instead of stacking two identical banners.
    const label = formatThreadLabel(threadId);
    showToast(`Merge conflict in ${label} — resolving automatically.`, 'warning', {
      key: `merge-conflict-${threadId}-${event.change_id ?? 'no-change'}`,
      onClick: () => focusThread(threadId),
    });
  }

  // Event-driven toast so every path that auto-spawns a hardening session
  // (Apply Now, Apply All, and the recovery sweep when a CC session ended
  // without /harden) notifies uniformly — mirrors MergeConflictDetected above.
  // The change applies automatically once hardening finishes; the toast is the
  // system-level "this is happening" cue, the in-thread initiator panel is the
  // local context. Keyed by thread (the event carries no change_id, and a
  // thread hardens one change at a time) so a re-emit refreshes one toast
  // instead of stacking.
  if (event.type === 'MissingHardeningDetected') {
    const label = formatThreadLabel(threadId);
    showToast(`Hardening required in ${label} — change will apply automatically after hardening.`, 'warning', {
      key: `missing-hardening-${threadId}`,
      onClick: () => focusThread(threadId),
    });
  }

  // Per-thread "events arrived" bell — fires for every event so subscribers
  // to this specific thread (focused ChatExchange / ThreadView /
  // activeStreamingBuffer) recompute. Streaming tokens land here exclusively
  // and don't reach the `threadMap` flush below.
  bumpThreadEvents(threadId);

  // Global `threadMap` flush ONLY when meta-shape actually changed. Skipping
  // it for streaming-only arrivals is the whole point — attentionThreadCount,
  // ThreadDrawer.ThreadList, every visible ChatExchange and every PromptInput
  // effect read `threadMap.value` in their subscribe path and would otherwise
  // re-execute per CC token. See `~/.claude/plans/generic-sparking-garden.md`.
  if (metaChanged) {
    scheduleThreadMapFlush();
  }
}

export function handleGlobalEvent(type: string, data: Record<string, unknown>): void {
  switch (type) {
    case 'NotificationCreated':
      // Bell badge only — the toast is driven by NotificationToastRequested (§4).
      handleNotificationSSE();
      break;

    case 'NotificationRead':
    case 'NotificationsAllRead':
      handleNotificationSSE();
      break;

    case 'PresenceCheck':
      // Engine asked every connected page for live presence so it can
      // decide whether to fan out the OS push. Pong only — the toast is
      // driven by NotificationToastRequested below. See
      // system-knowhow/notifications.md §3.
      handlePresenceCheck(data as unknown as PresenceCheckPayload);
      break;

    case 'NotificationToastRequested':
      // Engine decided to suppress the OS push (an active device pong'd in)
      // and is asking active pages to render the in-app toast instead. The
      // §4 row matrix (in showInAppNotificationToast) decides toast vs.
      // auto-read vs. no-op. See system-knowhow/notifications.md §4.
      handleNotificationToastRequested(data as unknown as NotificationToastRequestedPayload);
      break;

    case 'NativePushRequested':
      // Engine allowed the OS push (no active device) and is asking a connected
      // Tauri desktop app to render a NATIVE macOS banner — the WKWebView can't
      // receive the web push. Browser / PWA pages ignore it (handleNativePush-
      // Requested gates on isTauri). See system-knowhow/notifications.md §4.
      handleNativePushRequested(data as unknown as NativePushRequestedPayload);
      break;

    case 'PreferencesChanged':
    // `set_language` / `set_timezone` (chat-agent tools) write the preference
    // and emit LanguageSet / TimezoneSet but NOT PreferencesChanged, so without
    // these arms the cached `preferences` (and thus the locale/timezone shown in
    // the UI) would stay stale until reload. loadPreferences re-reads the full
    // map, covering both.
    case 'LanguageSet':
    case 'TimezoneSet':
      // loadPreferences sets `preferences` to `failed` via toFailed on error —
      // no extra surface needed here.
      void loadPreferences();
      break;

    case 'AppUiRefreshRequested':
      // Transient system event aggregated on `app`. The engine emits it
      // after every app coding-agent apply that touches an iframe-bundled
      // file; the SDK iframe of `app_id` reloads to pick up the merged
      // content. The matching handler in `handleTransientSideEffects` runs
      // only for ThreadEvent envelopes — this `handleGlobalEvent` branch
      // is the one that actually fires for the live SystemEvent SSE frame
      // (`{"type":"AppUiRefreshRequested","data":{"app_id":"..."}}`).
      void refreshAppUI(data.app_id as string | undefined);
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

    case 'ApplyAllBatchStarted': {
      // An Apply All batch started (possibly on another device). Reflect
      // "in progress" on the bulk buttons and mark every member as applying so
      // each pending row shows "Applying..." for the whole batch — not just the
      // one being merged right now. ChangeApplied/ChangeApplyFailed clear each
      // member id; ApplyAllBatchCompleted drops the bulk flag.
      applyAllInProgress.value = true;
      const changeIds = (data.change_ids ?? []) as string[];
      if (changeIds.length > 0) {
        applyingChangeIds.value = new Set([...applyingChangeIds.value, ...changeIds]);
      }
      break;
    }

    case 'ApplyAllBatchCompleted': {
      // Batch finished or was canceled — every member resolved as applied or
      // failed (a cancel marks the in-flight + queued members failed with
      // "Apply All canceled"). The per-change ChangeApplied/ChangeApplyFailed
      // handlers clear ids for members that emitted a thread event, but a
      // canceled batch's queued members never do — so clear the full
      // applied∪failed set here to drop any "Applying..." stragglers, then drop
      // the bulk in-progress flag.
      const applied = (data.applied ?? []) as string[];
      const failed = ((data.failed ?? []) as Array<{ change_id?: string }>)
        .map((f) => f.change_id)
        .filter((id): id is string => typeof id === 'string');
      const resolved = new Set<string>([...applied, ...failed]);
      if (resolved.size > 0 && applyingChangeIds.value.size > 0) {
        const next = new Set([...applyingChangeIds.value].filter((id) => !resolved.has(id)));
        if (next.size !== applyingChangeIds.value.size) applyingChangeIds.value = next;
      }
      applyAllInProgress.value = false;
      break;
    }

    case 'ChangesUpdated': {
      const pending = (data.pending ?? []) as Change[];
      const applied = (data.applied ?? []) as Change[];
      const totalPending = (data.total_pending as number) ?? 0;
      changes.value = { status: 'loaded', data: pending };
      appliedChanges.value = { status: 'loaded', data: applied };
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
      backupStatusVersion.value++;
      break;
    }

    case 'BackupFailed': {
      backupProgress.value = null;
      showToast(`Backup failed: ${String(data.error ?? 'Unknown error')}`, 'error');
      backupStatusVersion.value++;
      break;
    }

    // Restore events mirror the engine's authoritative restore_state — the SAME
    // shape getRestoreStatus() returns, so a reloaded page (which seeds from the
    // fetch) and a live page (which gets these) render identically.
    case 'RestoreProgress': {
      restoreState.value = {
        status: 'running',
        workspace_name: String(data.workspace_name ?? ''),
        phase: String(data.phase ?? ''),
        progress: Number(data.progress ?? 0),
        total: Number(data.total ?? 0),
      };
      break;
    }

    case 'RestoreCompleted': {
      restoreState.value = {
        status: 'completed',
        workspace_name: String(data.workspace_name ?? ''),
        workspace_path: String(data.workspace_path ?? ''),
      };
      showToast(`Workspace restored: ${String(data.workspace_name ?? '')}`, 'success');
      break;
    }

    case 'RestoreFailed': {
      restoreState.value = {
        status: 'failed',
        workspace_name: String(data.workspace_name ?? ''),
        error: String(data.error ?? 'Unknown error'),
      };
      showToast(`Restore failed: ${String(data.error ?? 'Unknown error')}`, 'error');
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
      // Loadable/showToast surfaces inside loadAllThreads + refreshThreadEvents
      // carry the failure if one happens; here `void` just acknowledges that
      // we don't need the promise back.
      void resyncLoadedThreads();
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

function clearComposeIfUnfocused(threadId: string): void {
  if (isComposeFocusedHere(threadId)) return;
  clearDraft(threadId);
}

/** Handle transient ThreadEvent types that trigger side effects (modals, refreshes). */
function handleTransientSideEffects(event: ThreadEvent | TransientEvent): void {
  switch (event.type) {
    case 'CredentialPromptRequested':
      try {
        openCredentialRequest(JSON.parse((event as { payload: string }).payload));
      } catch (e) {
        console.error('Failed to parse credential request:', e);
        showToast('Failed to handle credential request from engine', 'error');
      }
      break;

    case 'PluginInstallRequested':
      try {
        openPluginInstallRequest(JSON.parse((event as { payload: string }).payload));
      } catch (e) {
        console.error('Failed to parse plugin install request:', e);
        showToast('Failed to handle plugin install request from engine', 'error');
      }
      break;

    case 'PluginUninstallRequested':
      try {
        openPluginUninstallRequest(JSON.parse((event as { payload: string }).payload));
      } catch (e) {
        console.error('Failed to parse plugin uninstall request:', e);
        showToast('Failed to handle plugin uninstall request from engine', 'error');
      }
      break;

    case 'EmailConfirmRequested':
      try {
        const request = JSON.parse((event as { payload: string }).payload);
        landOnAccountsWithOverlay({ type: 'form', form: { type: 'email-confirm', request } });
        pushNavState();
      } catch (e) {
        console.error('Failed to parse email confirm request:', e);
        showToast('Failed to handle email confirm request from engine', 'error');
      }
      break;

    case 'PushNotificationRequested':
      void (async () => {
        try {
          const ok = await showConfirm(
            'Enable push notifications?',
            'Enable',
            { variant: 'default' }
          );
          if (!ok) return;
          await initPushSubscription();
          await toggleDevicePush(getDeviceId(), true);
        } catch (e) {
          showToast(`Failed to enable push notifications: ${errorDetail(e)}`, 'error');
        }
      })();
      break;

    case 'McpConsentPromptRequested':
      void (async () => {
        try {
          const { request_id, server_name, tool_name, arguments_summary } = JSON.parse((event as { payload: string }).payload);
          const msg = `**${server_name}** wants to call **${tool_name}**\n\n\`\`\`json\n${arguments_summary}\n\`\`\``;
          const ok = await showConfirm(msg, 'Allow', { variant: 'default' });
          await postMcpConsent(request_id, ok);
        } catch (e) {
          console.error('[SSE] Failed to handle MCP consent request:', e);
          showToast('Failed to send MCP consent response', 'error');
        }
      })();
      break;

    case 'FileRefreshRequested':
      // loadArtifacts sets `artifacts` to `failed` via toFailed on error.
      void loadArtifacts();
      break;

    case 'ToolResult': {
      const name = (event as { name: string }).name;
      if (['write_file', 'edit_file', 'copy_file', 'delete_file', 'import_file'].includes(name)) {
        void loadArtifacts();
      }
      break;
    }

    case 'AppUiCaptureRequested': {
      const e = event as { app_id: string; request_id: string };
      // captureAppUI owns its own console.warn telemetry + best-effort
      // postAppCapture surfaces (see apps.ts) — `void` here.
      void captureAppUI(e.app_id, e.request_id);
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
      const currentThreadId = focusedThreadId.value;
      const userMessages = currentThread?.pendingUserMessages.length ? [...currentThread.pendingUserMessages] : [];
      const movedMessages = userMessages.length > 0;
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
      // handleGlobalEvent has no bottom bump like handleThreadEvent does, so
      // we must fire the per-thread bumps here for both threads whose
      // pendingUserMessages just changed. Without these, the originating
      // thread keeps painting the just-moved 'Requesting...' row, and the
      // new CC thread doesn't render the seeded pendingUserMessages until
      // its first real event lands.
      if (movedMessages && currentThreadId) bumpThreadEvents(currentThreadId);
      bumpThreadEvents(e.cc_thread_id);
      break;
    }
  }
}

export { handleNavigationRequest } from './navigation-request';
