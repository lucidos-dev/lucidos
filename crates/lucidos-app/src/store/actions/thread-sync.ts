import { API } from '../../api/client';
import type { Change } from '../../api/client';
import { threadMap, focusedThreadId, changes, appliedChanges, applyingChangeIds, applyingNowThreadIds, applyAllInProgress, generatedTitleIds, codingAgentSessionVersion, setFocusedThread, archivingThreadIds, removingQueuedMessageIds, queuedMessageRemovalKey } from '../store';
import { memoryRebuildProgress, backupProgress, backupStatusVersion, backupPreferencesVersion, recoveryProgress, showConfirm, showToast, dismissToast, toasts, repoSource, TOAST_AUTO_DISMISS_MS } from '../store';
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
  handleNativePushDismiss,
  type NativePushDismissRequestedPayload,
} from './native-push';
import { addRestartGroup } from './chat-changes';
import {
  handleFrontendUpdateDeferred,
  handleFrontendUpdateStranded,
  handleEngineBuildStateChanged,
  type FrontendUpdateDeferredPayload,
  type FrontendUpdateStrandedPayload,
} from './engine-update';
import {
  handleFrontendPreviewStarted,
  handleFrontendPreviewStopped,
} from './frontend-preview';
import { changeToastMessage } from './changeToast';
import { scheduleServiceWorkerUpdateChecks } from '../../hooks/sw-update';
import { syncClientUpdateFromBuild } from './client-update';
import { loadPreferences } from './preferences';
import { loadArtifacts } from './artifacts';
import { refreshAppUI, captureAppUI } from './apps';
import { clearWipIfMatches } from './wipPreview';
import { openCredentialRequest } from './credentials';
import { openPluginInstallRequest } from './plugin-install';
import { openPluginUninstallRequest } from './plugin-uninstall';
import { openEmailConfirmRequest } from './email-confirm';
import { setDevicePushEnabled } from './push';
import { getDeviceId } from './devices';
import { focusThread } from './threads';
import { formatThreadLabel } from './thread-label';
import { refreshRepoView } from './repositories';
import { processSSEForReferences } from './entityReferences';
import { refreshThreadEvents, loadThreadEvents, forgetThreadEventsFailures, markLoadedThreadsStale } from './thread-loading';
import { refreshThreadList } from './thread-list-refresh';
import { applyRemoteCompose, pendingComposePuts, hasUnsentLocalDraft, clearSupersededDraft, noteComposeEpoch } from './compose';
import type { ComposeSelectionOverride } from '../composeSelections';
import { clearDraft, setDraft } from '../composeDrafts';
import { removeThreadNavEntries } from './thread-navigation';
import { isComposeFocusedHere } from '../../components/chat/promptFocus';
import { formatBytes } from '../../utils/formatBytes';
import { errorDetail } from '../../utils/errorDetail';
import { handleNavigationRequest, describeNavTarget } from './navigation-request';
import { applyEmbeddingModelStatus } from './backgroundActivity';
import type { EmbeddingModelStatus } from '../../api/types';

/** The nil UUID the engine stamps on a thread-less `NavigationRequested`. The
 *  SDK `lucidos.ui.navigate` app-iframe bridge (api/sdk.rs) emits it, being
 *  user-initiated and bound to no thread. */
const NIL_THREAD_ID = '00000000-0000-0000-0000-000000000000';

let eventSource: EventSource | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let repoChangesDebounce: ReturnType<typeof setTimeout> | null = null;

function markEventStreamStatus(status: 'connecting' | 'connected' | 'disconnected'): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.lucidosEventStream = status;
}

/** Set when `onerror` fires, consumed by the next `onopen`, so the resync runs
 *  only on RECONNECT. On the initial connect `loadAllThreads()` is already
 *  driving state via useStartup.ts. */
let needsResyncOnOpen = false;

/** In-flight resync coalescer. Multiple Lagged events, or back-to-back
 *  reconnects, collapse into one network round-trip. */
let resyncInFlight: Promise<void> | null = null;

// Events that clear optimistic Apply Now state. The apply completed or failed,
// the backend took over on a merge conflict, or CC resumed work.
const APPLY_NOW_CLEAR_EVENTS = new Set([
  'ChangeApplied', 'ChangeApplyFailed',
  'MergeConflictDetected', 'CodingAgentToolCalled', 'CodingAgentTextStreamed',
  // Reasoning is the EARLIEST agent-resumed signal, preceding text and tools,
  // so clear the stranded Apply-Now state on it too. A long reasoning pass
  // would otherwise hold that state minutes longer than its siblings.
  'CodingAgentThoughtStreamed',
  'CodingAgentUserMessageSent', 'CodingAgentPromptSent', 'MessageReceived',
]);

/** The preference keys the Backup page renders, mirroring the `PREF_BACKUP_*`
 *  constants in the engine's `core/backup/mod.rs`. Only a `PreferencesChanged`
 *  carrying one of them makes that page re-read. Every other key must leave its
 *  endpoints alone. */
const BACKUP_PREFERENCE_KEYS = new Set([
  'backup_provider',
  'backup_schedule',
  'backup_retention',
]);

/** Toast key for the merge-conflict banner. Shared by the
 *  MergeConflictDetected emitter and the terminal-event resolver. The same
 *  toast then updates in place, or is dismissed at a terminal state, rather
 *  than lingering as a stale warning. */
function mergeConflictToastKey(threadId: string, changeId: string | undefined): string {
  return `merge-conflict-${threadId}-${changeId ?? 'no-change'}`;
}

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

/** The id of the `MergeConflictDetected` event for a change, so the conflict
 *  toast can deep-link to the turn that reports it.
 *
 *  `MergeConflictDetected` is an exchange STARTER, so its turn's root carries
 *  that id as `data-event-id` (see `stampedEventIds`). The deep-link resolves
 *  straight onto the merge panel, needing no anchor re-targeting.
 *
 *  **Highest seq wins.** A Tier-2 to Tier-3 cascade emits two for one change,
 *  and the newer one carries the resolution the toast is talking about. Ranked
 *  by the map's KEY rather than by the last match: `thread.events` iterates in
 *  insertion order, so a backfill landing after a live SSE event puts an older
 *  seq last.
 *
 *  Both emit sites rank through here, so the banner and the resolved toast it
 *  turns into can never point at different panels. `changeId` is optional to
 *  match `mergeConflictToastKey`. The two conflict events a change-less pair
 *  produces share one toast, so they must share one landing too. */
function findMergeConflictEventId(threadId: string, changeId: string | undefined): string | undefined {
  const thread = threadMap.value.get(threadId);
  if (!thread) return undefined;
  let found: string | undefined;
  let foundSeq = -1;
  for (const [seq, event] of thread.events) {
    if (seq > foundSeq && event.type === 'MergeConflictDetected'
        && event.change_id === changeId && event._eventId) {
      found = event._eventId;
      foundSeq = seq;
    }
  }
  return found;
}

export function connectThreadEvents(): void {
  if (eventSource) return;

  const gen = ++sseGeneration;
  const url = `${API}/events`;
  markEventStreamStatus('connecting');
  const es = new EventSource(url);
  eventSource = es;

  es.onmessage = (msg) => {
    // Stale handler — a newer connection replaced this one. The old
    // EventSource was closed, but its queued message handler can still fire.
    if (gen !== sseGeneration) return;

    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(msg.data);
    } catch (err) {
      // Telemetry carve-out (.claude/rules/frontend.md): an unparseable SSE
      // frame arrives without user intent and names no user-facing operation,
      // so there is nothing honest to toast about. Self-recovery: the stream
      // stays open and the next frame is handled normally. Any state the
      // dropped frame carried is re-read by `resyncLoadedThreads` on the next
      // reconnect or wake. Logged rather than swallowed, so a malformed
      // envelope is diagnosable instead of looking like an event never sent.
      console.warn('[SSE] dropping unparseable frame', err);
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
    markEventStreamStatus('connected');
    // Only resync after a reconnect — on the initial connect, useStartup.ts
    // already loads thread state. Without the flag we'd double-fetch on every
    // page load.
    if (needsResyncOnOpen) {
      needsResyncOnOpen = false;
      // Terminal BackupCompleted and BackupFailed are ephemeral. Fired during
      // the SSE gap, they leave the UI on "Backing up" forever. Clear the
      // signal so the user can retry. An in-flight backup repopulates on the
      // next BackupProgress event, and a duplicate POST returns 409.
      backupProgress.value = null;
      // `resyncLoadedThreads` coalesces and surfaces its own failures, so
      // `void` here only acknowledges that the promise is not needed back.
      void resyncLoadedThreads();
    }
  };

  es.onerror = () => {
    // Stale handler: disconnectThreadEvents() already closed this EventSource
    // and connectThreadEvents() created a replacement. Without this guard the
    // old handler closes the NEW connection, via the module-scoped
    // `eventSource`, costing a 3s SSE gap on every iOS Safari PWA resume.
    if (gen !== sseGeneration) return;

    // Mark for resync. Events emitted during the gap never reach this tab, so
    // the next successful connect must refetch persisted state.
    needsResyncOnOpen = true;
    markEventStreamStatus('disconnected');

    es.close();
    eventSource = null;
    if (reconnectTimer) clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connectThreadEvents();
    }, 3000);
  };
}

/** Refetch thread metadata for every thread, and missed events for the focused
 *  one. Called when SSE drops + reconnects, or when the backend signals `Lagged`
 *  (its broadcast subscriber fell behind the buffer and dropped events).
 *  Without this, a tab that misses `ResponseGenerated` shows the "Thinking"
 *  spinner indefinitely while the backend has long since gone idle.
 *
 *  That spinner is repaired by the METADATA half. The drawer's status dot, its
 *  sections and its badges all read `meta.status`, which the one
 *  `loadAllThreads` request below refreshes for every thread it returns. The
 *  per-thread event fetch matters only for the transcript on screen. Every
 *  other loaded thread is marked stale here and refreshed when opened. */
export function resyncLoadedThreads(): Promise<void> {
  if (resyncInFlight) return resyncInFlight;
  resyncInFlight = (async () => {
    try {
      // Before the metadata read, because only a fetch STARTING after a mark
      // may clear it (see `staleMarkedAtToken`). A thread `loadAllThreads`
      // eagerly loads below must be on the far side of this line to clear its
      // own mark on landing.
      markLoadedThreadsStale();
      // Refresh thread-level metadata first, so any per-thread refresh sees the
      // authoritative state. `loadAllThreads` REJECTS on a failed GET and has
      // no Loadable or toast of its own. Letting that propagate would skip the
      // per-thread refresh below, which is what clears a stuck spinner after an
      // SSE gap. `refreshThreadList` never rejects, and owns the single keyed
      // card this shares with the resume sync.
      await refreshThreadList();
      // One request, for the thread on screen. The metadata read above is what
      // repairs the drawer, and `refreshStaleThreadEvents` consumes the rest of
      // the marks on focus.
      //
      // After the metadata read, deliberately, so the refresh sees the
      // authoritative state. Coalesced against a concurrent wake resync, and
      // `refreshThreadEvents` surfaces its own failures.
      const focused = focusedThreadId.value;
      if (focused && threadMap.value.get(focused)?.eventsLoaded) {
        await refreshThreadEvents(focused, { coalesce: true });
      }
    } finally {
      resyncInFlight = null;
    }
  })();
  return resyncInFlight;
}

export function disconnectThreadEvents(): void {
  // Bump generation BEFORE closing, so any onerror handler queued by the
  // close() call sees a stale generation and bails out.
  sseGeneration++;
  // An explicit disconnect means the caller is taking ownership of state
  // recovery. Do not let the next onopen also resync, or every real reconnect
  // doubles the per-thread refreshThreadEvents fan-out.
  needsResyncOnOpen = false;
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  for (const timer of applySessionEndedTimers.values()) clearTimeout(timer);
  applySessionEndedTimers.clear();
  markEventStreamStatus('disconnected');
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
  // event, and on transient projection refreshes such as ChildrenCountChanged
  // or CodingAgentDiffChanged. handleEvent overlays it onto thread.meta.
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

  // Track meta-shape changes across the whole handler, to gate the global
  // `threadMap` signal flush at the bottom. Per-thread event arrivals bump
  // `threadEventsBump` unconditionally. The wide flush fires only when a meta
  // field consumers care about actually changed. Without the gate, every CC
  // streaming token would re-execute attentionThreadCount and every visible
  // ChatExchange. See `store/threadActivity.ts`.
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
      handleTransientSideEffects(event, threadId);
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
    // The flag is also the ONLY thing that puts a thread into
    // `runResumeSync`'s failed-load retry set. Clearing it here is the thread's
    // last exit from that queue: with `eventsLoaded` still false it belongs to
    // neither collection, and nothing will fetch it again to retract its card.
    // This block's own premise, SSE delivering persisted events for this
    // thread, is the evidence that the failure is over.
    forgetThreadEventsFailures(threadId);
  }

  // seq from SSE: present (number > 0) for persisted events, absent for transient
  const effectiveSeq = seq ?? null;

  const handled = handleEvent(map, threadId, effectiveSeq, event, created, eventId, aggregate);
  if (handled.metaChanged) metaChanged = true;
  if (event.type === 'QueuedMessageRemoved') {
    const key = queuedMessageRemovalKey(threadId, event.removed_message_id);
    if (removingQueuedMessageIds.value.has(key)) {
      const next = new Set(removingQueuedMessageIds.value);
      next.delete(key);
      removingQueuedMessageIds.value = next;
    }
  }

  // Archive race guard. Every persisted SSE event carries the projection
  // snapshot AT EVENT EMIT TIME. A cascade archive emits CodingAgentIdled for
  // each descendant BEFORE the ThreadArchived row update lands, and that
  // intermediate aggregate still has section='inbox'. Without this guard
  // applyAggregateToMeta reverts the optimistic flip, so the row flies back to
  // Review until the matching ThreadArchived SSE lands and neighbours shift
  // twice. `archivingThreadIds` is the in-flight signal, set by
  // handleArchiveThread for every cascade member. It clears in that function's
  // finally, so post-archive SSE events apply their aggregate normally.
  if (aggregate && archivingThreadIds.value.has(threadId)) {
    if (thread.meta.section !== 'archived' || thread.meta.codingAgentProposed) {
      thread.meta.section = 'archived';
      thread.meta.codingAgentProposed = false;
      metaChanged = true;
    }
  }

  // **Compose-clear on a peer's send yields ONLY to the user's own unsent
  // work.** Authorship, not DOM focus, is the guard. Two arms:
  //   1. Origin-device echo. A send or discard from this device already mutated
  //      local compose state synchronously. The later SSE echo would blank text
  //      typed since, so drop it.
  //   2. Unsent local draft. A non-empty draft this device authored, not since
  //      submitted, is the user's unsent intent. It must never be blanked by an
  //      inbound echo, the same `hasUnsentLocalDraft` invariant
  //      stageDraftFromApi and applyRemoteCompose enforce. A SUPERSEDED draft,
  //      whose text this very message carries, is not unsent work and does
  //      clear.
  //
  // Deliberately NOT gated on `isComposeFocusedHere`. A focus guard would also
  // keep a SERVER-ORIGINATED draft the user never typed. A follow-up drafted on
  // a peer, synced here, then sent there, would sit as a ghost draft.
  // `hasUnsentLocalDraft` is the correct line: false for a synced draft, true
  // the moment the user types.
  if (event.type === 'MessageReceived') {
    if (thread.meta.state !== 'active') { thread.meta.state = 'active'; metaChanged = true; }
    if (!isFromThisDevice(event) && !hasUnsentLocalDraft(threadId)) clearDraft(threadId);
  }
  // A free-form answer to a pending question is a submitted draft that becomes
  // no MessageReceived, chat/process/run.rs rerouting the typed text straight
  // to UserQuestionAnswered. The arm above never sees it, so without this the
  // answered draft would linger.
  //
  // Scoped to the superseded case. Unlike a send, a question answer clears the
  // shared draft server-side only when the submitted text IS that draft. An
  // unrelated draft here must survive rather than diverge. The server's paired
  // ThreadComposeChanged supplies the other half of the supersede test, and
  // whichever frame lands second completes the clear.
  if (event.type === 'UserQuestionAnswered' && seq !== null) {
    clearSupersededDraft(threadId);
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
  // ThreadArchived deliberately does NOT touch `meta.state`. The compose state
  // machine is orthogonal to archive routing: an archived thread stays at
  // state='active' and only flips `archive_state` and `meta.section`. The
  // archive race guard above handles the section flip for cascade members, and
  // `applyAggregateToMeta` for direct archives. Both key off `archive_state`.

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
  handleTransientSideEffects(event, threadId);

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
    // No Refresh button here. At ChangeApplied time the rebuilt frontend is not
    // ready, the build-watch still running `vite build`, so a Refresh now would
    // reload the OLD build. The genuine affordance is the New-version toast
    // `surfaceUpdateToast` surfaces (store/actions/client-update.ts). The
    // build-id check drives it once the rebuilt sw.js is served, fired on the
    // new worker's activation and nudged by
    // scheduleServiceWorkerUpdateChecks().
    showToast(changeToastMessage('Applied', threadId, desc), 'success', {
      key: applyKey,
      onClick: () => focusThread(threadId),
      autoDismissMs: TOAST_AUTO_DISMISS_MS,
    });
    // Record the restart state immediately from the thread event, rather than
    // waiting for the separate ChangesUpdated system event. If ChangesUpdated is
    // missed (SSE drop, Vite reload race), this is what lights the badge.
    if (requiresRestart) {
      const commits = event.commits ?? [];
      const threadTitle = event.thread_title ?? threadMap.value.get(threadId)?.meta.title ?? 'Untitled thread';
      addRestartGroup({ threadId, threadTitle, commits });
    }
    if (clientUpdate) {
      // Do not light the badge here. It shares the toast's single source of
      // truth, the build-id check in `syncClientUpdateFromBuild`, so badge and
      // toast cannot disagree or appear out of order. At ChangeApplied time the
      // rebuilt bundle is not served yet, so an eager badge would lead the real
      // update. Nudge the SW to pick up the rebuilt /sw.js instead, its
      // activation re-running the build-id check and lighting BOTH together.
      //
      // For a frontend-only Apply the engine re-snapshots its served dist
      // in-process (engine::frontend_refresh). The served sw.js then advances
      // without a respawn, and this nudge is what surfaces it.
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

  // The merge-conflict banner is a sticky warning, with no auto-dismiss. Once
  // the conflict reaches a terminal state it must stop claiming to be still
  // resolving, so transition it in place. Guarded on the toast already
  // existing, since showToast(key) would otherwise CREATE a banner and a plain
  // apply would spawn a spurious resolved toast. ChangeApplied updates it to
  // resolved. ChangeApplyFailed and ChangeDiscarded dismiss it, the terminal
  // toast above already carrying that outcome.
  if ((event.type === 'ChangeApplied' || event.type === 'ChangeApplyFailed' || event.type === 'ChangeDiscarded')
      && event.change_id) {
    const conflictKey = mergeConflictToastKey(threadId, event.change_id);
    if (toasts.value.some((t) => t.key === conflictKey)) {
      if (event.type === 'ChangeApplied') {
        // Same landing as the banner it replaces in place. This is one toast
        // the reader watched change its wording, so tapping it after the
        // resolution must still open the conflict turn. Resolved at tap time
        // from the thread's own events, as the banner below does. The two can
        // then never disagree, and no per-toast state is kept alive.
        const changeId = event.change_id;
        showToast(`Merge conflict in ${formatThreadLabel(threadId)} — resolved.`, 'success', {
          key: conflictKey,
          onClick: () => focusThread(threadId, {
            targetEventId: findMergeConflictEventId(threadId, changeId) ?? null,
          }),
          autoDismissMs: TOAST_AUTO_DISMISS_MS,
        });
      } else {
        dismissToast(conflictKey);
      }
    }
  }

  // The "hardening required — change will apply automatically after hardening"
  // banner is a sticky warning (no auto-dismiss). The change applies
  // automatically once hardening finishes, so a ChangeApplied for this thread
  // IS the "done" signal — transition the banner in place to "applied". Guarded
  // on the toast already existing so a plain (non-hardening) apply never spawns
  // a spurious "Hardening applied" toast. ChangeApplyFailed / ChangeDiscarded
  // just dismiss it — the terminal toast above already carries that outcome.
  // Keyed by thread (no change_id — a thread hardens one change at a time),
  // matching the MissingHardeningDetected emit below. Mirrors the merge-conflict
  // "resolved" transition above.
  if (event.type === 'ChangeApplied' || event.type === 'ChangeApplyFailed' || event.type === 'ChangeDiscarded') {
    const hardeningKey = `missing-hardening-${threadId}`;
    if (toasts.value.some((t) => t.key === hardeningKey)) {
      if (event.type === 'ChangeApplied') {
        showToast(`Hardening applied for ${formatThreadLabel(threadId)}.`, 'success', {
          key: hardeningKey,
          onClick: () => focusThread(threadId),
          autoDismissMs: TOAST_AUTO_DISMISS_MS,
        });
      } else {
        dismissToast(hardeningKey);
      }
    }
  }

  // After apply/discard/revert, reveal the app header on mobile so the result
  // is readable with full navigation visible. The transcript is NOT moved: the
  // resolution card lands below whatever the reader is looking at, and the
  // chevron is how they go to it.
  if (event.type === 'ChangeApplied' || event.type === 'ChangeDiscarded' || event.type === 'ChangeReverted') {
    if (threadId === focusedThreadId.value) {
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
    // Event-driven toast, so all three engine paths notify uniformly. Fires
    // whatever the focus or visibility: the panel is local context, the toast
    // a system-level cue. Keyed by thread and change, so a Tier-2 to Tier-3
    // cascade refreshes one toast rather than stacking two identical banners.
    //
    // The tap deep-links to the conflict event, not just to its thread. The
    // panel is what the toast announces, and a plain focus would land the
    // reader at the thread's saved scroll with no conflict on screen. Ranked
    // through the same helper the resolved transition uses, so the one toast
    // cannot change where it goes when it changes its wording. The arriving id
    // is the fallback for a frame carrying no `event_id`.
    const label = formatThreadLabel(threadId);
    showToast(`Merge conflict in ${label} — resolving automatically.`, 'warning', {
      key: mergeConflictToastKey(threadId, event.change_id),
      onClick: () => focusThread(threadId, {
        targetEventId: findMergeConflictEventId(threadId, event.change_id) ?? eventId ?? null,
      }),
    });
  }

  // Event-driven toast, so every path that auto-spawns a hardening session
  // notifies uniformly. Mirrors MergeConflictDetected above. The change applies
  // automatically once hardening finishes. The toast is the system-level cue,
  // the in-thread initiator panel the local context. Keyed by thread, so a
  // re-emit refreshes one toast instead of stacking. The event carries no
  // change_id, and a thread hardens one change at a time.
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

  // Global `threadMap` flush ONLY when meta-shape actually changed. Skipping it
  // for streaming-only arrivals is the whole point. attentionThreadCount,
  // ThreadDrawer.ThreadList, every visible ChatExchange and every PromptInput
  // effect read `threadMap.value` in their subscribe path, so they would
  // otherwise re-execute per CC token.
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
      // Engine allowed the OS push, no active device having pong'd, and asks a
      // connected Tauri desktop app to render a NATIVE macOS banner. The
      // WKWebView cannot receive the web push. Browser and PWA pages ignore it,
      // the handler gating on isTauri. See system-knowhow/notifications.md §4.
      handleNativePushRequested(data as unknown as NativePushRequestedPayload);
      break;

    case 'NativePushDismissRequested':
      // A notification was read, here or on another device, and the engine asks
      // a connected Tauri desktop app to REMOVE its delivered native banners.
      // Browser and PWA pages ignore it, the handler gating on isTauri: the
      // open web cannot silently remove a Web Push banner. See
      // system-knowhow/notifications.md §4.
      handleNativePushDismiss(data as unknown as NativePushDismissRequestedPayload);
      break;

    case 'PreferencesChanged':
      // A peer device may have just dismissed the client-refresh toast globally
      // via the `client_refresh_dismissed_build` preference. So reload
      // preferences and THEN re-derive the client-update surface, to hide the
      // toast here too. Ordered, because syncClientUpdateFromBuild reads the
      // reloaded `preferences` signal through `wasSwUpdateDismissed`: it must
      // run after loadPreferences resolves, or it reads the stale value.
      // Idempotent and self-correcting. The engine-switch toast needs no
      // equivalent, its version-status poll hiding it once `wasSwitchDismissed`
      // reads true. loadPreferences sets `preferences` to `failed` on error.
      void loadPreferences().then(() => syncClientUpdateFromBuild()).catch(() => { /* best-effort re-derive */ });
      // The Backup page does NOT read its three values out of the preferences
      // cache: they arrive from `/backup/schedule`, `/backup/providers` and
      // `/backup/retention`, which is where the provider's connected/ready
      // verdict comes from too. So reloading the cache above leaves that page
      // stale, and only a re-read of those endpoints fixes it. Keyed, because a
      // theme or model change must not hit the backup endpoints. `value` is
      // null when a preference was deleted (reset to default), which is a
      // change like any other: what the page shows has to move either way.
      if (BACKUP_PREFERENCE_KEYS.has(String(data.key ?? ''))) {
        backupPreferencesVersion.value++;
      }
      break;
    // The `set_language` and `set_timezone` chat-agent tools write the
    // preference and emit LanguageSet or TimezoneSet, but NOT
    // PreferencesChanged. Without these arms the cached `preferences` would
    // stay stale until reload. loadPreferences re-reads the full map.
    case 'LanguageSet':
    case 'TimezoneSet':
      void loadPreferences();
      break;

    case 'AppUiRefreshRequested':
      // Transient system event aggregated on `app`. The engine emits it after
      // every app coding-agent apply touching an iframe-bundled file. The SDK
      // iframe of `app_id` reloads to pick up the merged content. The matching
      // handler in `handleTransientSideEffects` runs only for ThreadEvent
      // envelopes. This branch is the one that fires for the live SystemEvent
      // SSE frame.
      void refreshAppUI(data.app_id as string | undefined);
      break;

    case 'FrontendUpdateDeferred':
      // Dev-only transient signal: a frontend-only Apply couldn't advance the
      // served client in-process because an engine version change is pending
      // (engine::frontend_refresh INV-A). The change ships on the next Switch;
      // surface a keyed hint so it reads as queued, not ignored.
      handleFrontendUpdateDeferred(data as unknown as FrontendUpdateDeferredPayload);
      break;

    case 'FrontendUpdateStranded':
      // Dev-only transient signal: a frontend-only Apply rebuilt, but the
      // engine serves a dist/ that nothing republishes into. The change can
      // never reach this client and no Switch will deliver it, the rebuild wait
      // in engine::frontend_refresh having timed out. Warn, with the served
      // path, rather than staying silent.
      handleFrontendUpdateStranded(data as unknown as FrontendUpdateStrandedPayload);
      break;

    case 'ServedFrontendAdvanced':
      // Dev-only transient signal: THIS engine advanced its served-frontend
      // snapshot to the checkout-shared dist/ after a PEER workspace's
      // frontend-only Apply. Re-run the build-id check, so the Refresh badge
      // and toast surface without a manual restart. Idempotent and
      // self-correcting, so no payload is needed.
      void syncClientUpdateFromBuild();
      break;

    case 'FrontendPreviewStarted':
      // Dev-only transient signal: the engine brought up the Vite dev server
      // showing a coding-agent worktree's frontend (engine::frontend_preview).
      // The payload carries the PORT, never a URL, because only this page knows
      // which host the user reached the workspace under.
      handleFrontendPreviewStarted(data as { thread_id?: string; port?: number });
      break;

    case 'FrontendPreviewStopped':
      handleFrontendPreviewStopped(data as { thread_id?: string });
      break;

    case 'EngineBuildStateChanged':
      // Dev-only transient POKE: the engine's background rebuild changed state.
      // Re-run the authoritative version-status read, so the building spinner
      // and Switch badge track a real build over SSE. The throttled poll alone
      // is not enough, iOS suspending it on a backgrounded PWA.
      handleEngineBuildStateChanged();
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

    case 'EmbeddingModelStatusChanged': {
      // Transient frame from the engine's background embedding-model loader:
      // download progress and every transition between downloading / loading /
      // ready / waiting / failed. Same shape as the
      // `/memory/embedding-model-status` snapshot useStartup reads, so this is
      // a straight assignment with no translation.
      // Routed through the action rather than assigning the signal here, so the
      // freshness counter an in-flight snapshot read compares against cannot be
      // bypassed (see `applyEmbeddingModelStatus`).
      applyEmbeddingModelStatus({
        model_id: String(data.model_id ?? ''),
        load_state: data.load_state as EmbeddingModelStatus['load_state'],
      });
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
      // Batch finished or was canceled, so every member resolved as applied or
      // failed. A cancel marks the in-flight and queued members failed. The
      // per-change handlers clear ids for members that emitted a thread event,
      // but a canceled batch's queued members never do. So clear the full
      // applied and failed set here to drop any stragglers, then drop the bulk
      // in-progress flag.
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
      changes.value = { status: 'loaded', data: pending };
      appliedChanges.value = { status: 'loaded', data: applied };
      // `changesHasMore` tracks whether more APPLIED changes are pageable, and
      // the ChangesUpdated payload carries no `has_more_applied`. Its
      // `total_pending` is literally `pending.len()`, so a pending-count
      // comparison is always false. Deriving it here kills the applied-list
      // infinite scroll: leave the flag to refreshChangesState and
      // loadMoreChanges, which read the real field.
      //
      // restartRequired is deliberately untouched here. Stale SSE values would
      // otherwise drop the restart state while one is genuinely pending.
      //
      // Debounce the repo-scoped refresh: ChangesUpdated fires globally.
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
      backupStatusVersion.value++;
      break;
    }

    case 'BackupFailed': {
      backupProgress.value = null;
      showToast(`Backup failed: ${String(data.error ?? 'Unknown error')}`, 'error');
      backupStatusVersion.value++;
      break;
    }

    // Restore is no longer an engine SSE concern — it runs in the workspace
    // picker (gateway control plane), which polls its own restore-status. The
    // engine's `Restore*` events were removed with the Settings restore UI.

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
      // `resyncLoadedThreads` toasts a genuine failure itself (and stays silent
      // on transient wake noise), so `void` just acknowledges that we don't
      // need the promise back.
      void resyncLoadedThreads();
      break;
    }

    // ThreadComposeChanged is the SSE-only ephemeral notification emitted on
    // every compose PUT. Routed to compose.ts which writes the threadMap
    // entry's compose fields. Three guards layered together:
    //   1. origin_device_id. The server rebroadcasts to every device including
    //      the originator, so ignore our own echo. This suppresses only a
    //      PRESENT origin equal to self. A broadcast with an ABSENT origin
    //      bypasses the check, and applyRemoteCompose's own guard is the
    //      backstop for the dangerous empty-payload case (see
    //      docs/plans/2026-06-28-drafts-sse-empty-clear-guard.md). A non-empty
    //      absent-origin update still applies, carrying content, which is why
    //      this check does not break on an absent origin.
    //   2. pendingComposePuts. A debounced PUT may already be in flight with
    //      newer text, which the SSE event for our previous PUT would clobber.
    //   3. focused-textarea. With the user mid-keystroke on this thread's
    //      input, dropping a peer's NON-empty update beats moving the cursor.
    //      It must NOT drop a peer's EMPTY clear: that is the sent-elsewhere
    //      signal, and gating it on focus preserved the peer's draft in a
    //      focused but untyped textarea. applyRemoteCompose's own
    //      hasUnsentLocalDraft guard still protects unsent local work.
    case 'ThreadComposeChanged': {
      const id = data.id as string;
      // Recorded BEFORE either guard below. The *compose epoch* is a fact about
      // what the engine holds, not draft content. Neither our own echo nor a
      // write in flight is a reason to ignore it. The device with a write in
      // flight needs it most: its next write is fenced against this value, and
      // learning it here saves a 412 round trip after every send.
      noteComposeEpoch(id, data.compose_epoch as number | undefined);
      const originDeviceId = data.origin_device_id as string | undefined;
      if (originDeviceId && originDeviceId === getDeviceId()) break;
      if (pendingComposePuts.has(id)) break;
      const text = (data.text as string) ?? '';
      const imageHashes = Array.isArray(data.image_hashes) ? data.image_hashes as string[] : [];
      const modeRaw = data.mode as string | undefined;
      const mode = modeRaw === 'claude_code' ? 'claude_code' : modeRaw === 'lucidos' ? 'lucidos' : null;
      const isEmptyClear = text === '' && imageHashes.length === 0 && mode === null;
      if (!isEmptyClear && isComposeFocusedHere(id)) break;
      applyRemoteCompose(id, {
        text,
        image_hashes: imageHashes,
        mode,
        // Per-draft dropdown selection, DB-backed, hydrated into
        // composeSelections so a peer's change syncs. Absent means
        // setComposeSelectionFromServer clears any stale local entry, the DB
        // being authoritative. An in-flight local pick is already protected by
        // the pendingComposePuts guard above.
        selection: (data.selection as ComposeSelectionOverride | null | undefined),
      });
      break;
    }
  }
}

/** True when an inbound thread event was emitted by this browser. Per-event
 *  field: `MessageReceived.device_id` or `ThreadDiscarded.actor.device_id`. A
 *  match means the local mutating action already updated state synchronously,
 *  so applying the SSE echo would clobber keystrokes typed since. */
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

/** Tool names whose `ToolResult` means `data/` may have changed, so the Files
 *  list and any open file preview must re-read.
 *
 *  The five file tools are the obvious members. `bash_output` is here for a
 *  different reason: a background task writes to `data/` UNSTAGED by design, so
 *  a long-running job can let apps see partial output as it lands (see
 *  `engine/tools/python.rs`). No `Artifact*` or `DataFile*` event is emitted
 *  for those writes, leaving a drain as the only signal output has appeared.
 *
 *  **Plain `run_bash` is deliberately absent.** Its tool description forbids
 *  writing to `data/`, which is `run_python`'s job. Every entry here costs a
 *  full `data/` walk server-side via `list_artifacts`, not worth paying after
 *  each curl and ls. A bash write to `data/` is tool misuse, and the header
 *  Refresh button covers it. */
const ARTIFACT_REFRESHING_TOOLS = [
  'write_file', 'edit_file', 'copy_file', 'delete_file', 'import_file', 'bash_output',
];

/** Handle transient ThreadEvent types that trigger side effects (modals, refreshes).
 *
 *  `sourceThreadId` is the thread the event was emitted on. It scopes
 *  `NavigationRequested`, so a navigate from a sibling thread cannot hijack
 *  the page the user is viewing. */
function handleTransientSideEffects(event: ThreadEvent | TransientEvent, sourceThreadId: string): void {
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
        openEmailConfirmRequest(JSON.parse((event as { payload: string }).payload));
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
          // The same entry point both settings toggles use. Setting the device
          // flag unconditionally instead would leave a refused permission with
          // `push_enabled = true` and no subscription row. That is the
          // divergence `refreshPushSubscription` repairs on the next load.
          await setDevicePushEnabled(getDeviceId(), true);
        } catch (e) {
          showToast(`Failed to enable push notifications: ${errorDetail(e)}`, 'error');
        }
      })();
      break;

    // Chat MCP consent moved to the persisted in-thread `McpPermissionRequested`
    // permission card (rendered in ChatExchange via PermissionCard), replacing
    // the old transient `McpConsentPromptRequested` + showConfirm modal.

    // A tool call that may have changed `data/`. `loadArtifacts` refreshes the
    // Files list AND bumps `artifactRevision`, which cache-busts an open file
    // preview. This arm is what makes an agent edit show up on its own.
    // loadArtifacts sets `artifacts` to `failed` via toFailed on error.
    case 'ToolResult': {
      const name = (event as { name: string }).name;
      if (ARTIFACT_REFRESHING_TOOLS.includes(name)) {
        void loadArtifacts();
      }
      break;
    }

    // The background task finished, so its last writes have landed. Same
    // reasoning as `bash_output` in ARTIFACT_REFRESHING_TOOLS above.
    case 'BackgroundBashCompleted':
      void loadArtifacts();
      break;

    case 'AppUiCaptureRequested': {
      const e = event as { app_id: string; request_id: string };
      // captureAppUI owns its own console.warn telemetry + best-effort
      // postAppCapture surfaces (see apps.ts) — `void` here.
      void captureAppUI(e.app_id, e.request_id);
      break;
    }

    case 'NavigationRequested': {
      let nav;
      try {
        nav = JSON.parse((event as { payload: string }).payload);
      } catch (e) {
        console.error('[SSE] Failed to parse navigation request:', e);
        showToast('Failed to handle navigation request from engine', 'error');
        break;
      }
      // Device scope: an agent navigate (navigate_ui) carries the originating
      // device — the device that sent the prompt that triggered the turn (engine
      // stamps it in execute_navigate_ui). Such a navigate must act ONLY on that
      // device, not every device viewing the thread. So if the event names a
      // device that isn't this one, drop it entirely — no navigate, no offer.
      // Navigations with no device actor (trigger/background turns; the SDK
      // app-iframe nil-thread path) fall through to the thread/app scoping below.
      const navActor = (event as { actor?: { kind?: string; device_id?: string } }).actor;
      if (navActor?.kind === 'device' && navActor.device_id !== getDeviceId()) break;
      // Scope. A navigate acts on this page directly only when it originates
      // from the thread the user is viewing, or from an app iframe. The SDK
      // `lucidos.ui.navigate` path emits on the nil thread (api/sdk.rs), being
      // user-initiated and bound to no thread, so it always applies.
      const focused = focusedThreadId.value;
      const fromApp = sourceThreadId === NIL_THREAD_ID;
      if (fromApp || sourceThreadId === focused) {
        // Source label for any "couldn't open" toast downstream: which thread
        // asked, or the app iframe — so the error says where it came from
        // instead of swallowing it.
        const source = fromApp ? 'an app' : formatThreadLabel(sourceThreadId);
        handleNavigationRequest(nav, { source });
        break;
      }
      // Off-focus: a navigate from a thread the user isn't viewing must NOT
      // hijack the page. Offer to jump instead of silently dropping it — this
      // preserves "open X when you're done" from a background/sibling/trigger
      // thread. Tapping Open lands on BOTH the source thread (the context that
      // asked) and the navigate target. Keyed per source thread so repeated
      // navigates refresh one offer instead of stacking.
      const label = formatThreadLabel(sourceThreadId);
      showToast(`${label} wants to open ${describeNavTarget(nav)}`, 'info', {
        key: `nav-offer-${sourceThreadId}`,
        action: {
          label: 'Open',
          onClick: () => {
            dismissToast(`nav-offer-${sourceThreadId}`);
            focusThread(sourceThreadId);
            handleNavigationRequest(nav, { source: label });
          },
        },
      });
      break;
    }

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
      // `handleThreadEvent`'s bottom bump covers only the thread the event
      // ARRIVED on, and the transient-event path above it returns first. So
      // fire the per-thread bumps here, for both threads whose
      // pendingUserMessages just changed. Without them, the originating thread
      // keeps painting the just-moved 'Requesting' row, and the new CC thread
      // renders no seeded pendingUserMessages until its first real event.
      if (movedMessages && currentThreadId) bumpThreadEvents(currentThreadId);
      bumpThreadEvents(e.cc_thread_id);
      break;
    }
  }
}

export { handleNavigationRequest } from './navigation-request';
