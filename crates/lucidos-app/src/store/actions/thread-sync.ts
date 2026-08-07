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
import { initPushSubscription } from './push';
import { getDeviceId, toggleDevicePush } from './devices';
import { scrollToBottom } from '../../components/chat/scrollState';
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

/** The nil UUID the engine stamps on a thread-less `NavigationRequested` —
 *  emitted by the SDK `lucidos.ui.navigate` app-iframe bridge (api/sdk.rs),
 *  which is user-initiated and not bound to any thread. */
const NIL_THREAD_ID = '00000000-0000-0000-0000-000000000000';

let eventSource: EventSource | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let repoChangesDebounce: ReturnType<typeof setTimeout> | null = null;

function markEventStreamStatus(status: 'connecting' | 'connected' | 'disconnected'): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.lucidosEventStream = status;
}

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
  // Reasoning is the EARLIEST "agent resumed working" signal (it precedes
  // text/tools), so clear the stranded Apply-Now state on it too — otherwise a
  // long reasoning pass holds the state ~minutes longer than its siblings would.
  'CodingAgentThoughtStreamed',
  'CodingAgentUserMessageSent', 'CodingAgentPromptSent', 'MessageReceived',
]);

/** The preference keys the Backup page renders, mirroring the `PREF_BACKUP_*`
 *  constants in the engine's `core/backup/mod.rs`. A `PreferencesChanged`
 *  carrying one of them is the only kind that page has to re-read for; every
 *  other key (theme, model, locale) must leave its endpoints alone. */
const BACKUP_PREFERENCE_KEYS = new Set([
  'backup_provider',
  'backup_schedule',
  'backup_retention',
]);

/** Toast key for the "merge conflict — resolving automatically" banner. Shared
 *  by the MergeConflictDetected emitter and the terminal-event resolver so the
 *  same toast updates in place (→ "resolved") or is dismissed when the conflict
 *  reaches a terminal state, instead of lingering forever as a stale warning. */
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
      // stays open and the next frame is handled normally, and any state the
      // dropped frame carried is re-read by the resync on the next reconnect /
      // wake (`resyncLoadedThreads`). Logged rather than swallowed so a
      // malformed envelope is diagnosable instead of looking like an event the
      // backend never sent.
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
      // Terminal BackupCompleted/BackupFailed are ephemeral: if they fired during
      // the SSE gap, the UI would stay "Backing up..." forever. Clear the signal
      // so the user can retry; an in-flight backup will repopulate on the next
      // BackupProgress event, and a duplicate POST returns 409 with a clear toast.
      backupProgress.value = null;
      // `resyncLoadedThreads` coalesces, and surfaces its own failures (the
      // thread-list refresh and each per-thread refresh both toast a genuine
      // error and stay silent on transient wake noise), so `void` here just
      // acknowledges we don't need the promise back.
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
 *  That spinner is repaired by the METADATA half: the drawer's status dot, its
 *  sections and its badges all read `meta.status`, which the one `loadAllThreads`
 *  request below refreshes for every thread it returns. The per-thread event
 *  fetch matters only for the transcript on screen, so every other loaded thread
 *  is marked stale here and refreshed when the user opens it. */
export function resyncLoadedThreads(): Promise<void> {
  if (resyncInFlight) return resyncInFlight;
  resyncInFlight = (async () => {
    try {
      // Before the metadata read, because only a fetch that STARTS after a mark
      // may clear it (see `staleMarkedAtToken`): a thread `loadAllThreads`
      // eagerly LOADS below (a newly-active one) has to be on the far side of
      // this line to clear its own mark on landing.
      markLoadedThreadsStale();
      // Refresh thread-level metadata (status, section, message_count) first
      // so any per-thread refresh sees the authoritative state. `loadAllThreads`
      // REJECTS on a failed GET and has no Loadable or toast of its own, and
      // letting that propagate would skip the per-thread refresh below, which is
      // what clears a stuck "Thinking" spinner after an SSE gap (the very
      // failure this function exists to repair). `refreshThreadList` never
      // rejects, and owns the single keyed card this shares with the resume
      // sync, so the two report one failure of one request identically.
      await refreshThreadList();
      // One request, for the thread on screen. This used to be one per loaded
      // thread, four at a time, which on a large workspace an SSE drop re-ran in
      // full on every 3s reconnect, down a link that had just come back. The
      // metadata read above is what actually repairs the drawer; the rest of the
      // marks are consumed by `refreshStaleThreadEvents` on focus.
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
    // The flag is also the ONLY thing that puts a thread into `runResumeSync`'s
    // failed-load retry set, so clearing it here is the thread's last exit from
    // that queue: with `eventsLoaded` still false it now belongs to neither
    // collection, and nothing will fetch it again to retract its card. This
    // block's own premise (SSE is delivering persisted events for this thread)
    // is exactly the evidence that the failure is over, so retract it here.
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
  if (handled.clearedPendingUserMessage && threadId === focusedThreadId.value) {
    // Server confirmation swaps the optimistic row for the persisted user
    // event. Keep the focused transcript pinned to that replacement so the
    // just-sent follow-up does not appear to vanish until work starts.
    scrollToBottom();
  }

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

  // Compose-clear on a peer's send must yield ONLY to the user's own unsent
  // work — authorship, not DOM focus, is the guard:
  //   1. Origin-device echo: when the send/discard came from this device,
  //      sendCompose/discardCompose already mutated local compose state
  //      synchronously. The SSE echo arrives later and would blank any text
  //      the user has started typing for the next message — drop it.
  //   2. Unsent local draft: a non-empty draft this device authored, whose text
  //      has NOT since been submitted, is the user's unsent intent and must
  //      never be blanked by an inbound echo — the same `hasUnsentLocalDraft`
  //      empty-clear invariant stageDraftFromApi and applyRemoteCompose enforce.
  //      This covers the ACTIVE-thread follow-up draft the old
  //      `isComposeFocusedHere`-only guard missed: an echoed MessageReceived
  //      whose device_id didn't match (e2e / cross-device) wiped a just-typed
  //      follow-up after the user navigated away — the value='' face of
  //      drafts.spec.ts:65 (docs/plans/2026-06-27-mobile-webkit-shard-contention.md).
  //      A SUPERSEDED draft (this very message carries its text) is not unsent
  //      work and does clear — that's how a draft submitted from another device
  //      stops haunting this one.
  // Deliberately NOT gated on `isComposeFocusedHere`: a focus guard also keeps a
  // SERVER-ORIGINATED (synced-from-peer) draft the user never typed — so a
  // follow-up drafted on device A, synced here, then sent by A stayed as a ghost
  // draft in this device's focused textarea. hasUnsentLocalDraft is the correct
  // line (it's false for a synced draft, true the moment the user types), so the
  // backend's own compose_text='' clear on MessageReceived is mirrored here
  // regardless of focus.
  if (event.type === 'MessageReceived') {
    if (thread.meta.state !== 'active') { thread.meta.state = 'active'; metaChanged = true; }
    if (!isFromThisDevice(event) && !hasUnsentLocalDraft(threadId)) clearDraft(threadId);
  }
  // A free-form answer to a pending question is a submitted draft that never
  // becomes a MessageReceived — chat/process/run.rs reroutes the typed text
  // straight to UserQuestionAnswered — so the arm above never sees it, and
  // without this the draft that was answered with would linger. Scoped to the
  // superseded case: unlike a send, a question answer does not clear the
  // thread's shared draft server-side unless the submitted text IS that draft
  // (see the projection's UserQuestionAnswered arm), so an unrelated draft here
  // must survive rather than diverge from the server. The server's paired
  // ThreadComposeChanged supplies the other half of the supersede test; whichever
  // of the two frames lands second completes the clear.
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
    // No Refresh button here: at ChangeApplied time the rebuilt frontend isn't
    // ready yet (in --built mode the build-watch runs `vite build` over the next
    // few seconds), so a Refresh now would just reload the OLD build. The genuine
    // "ready to refresh" affordance is the "New version available → Refresh" toast
    // surfaced by `surfaceUpdateToast` (store/actions/client-update.ts), driven by
    // the build-id check once the rebuilt sw.js is actually served — fired on the
    // new worker's activation and nudged promptly by scheduleServiceWorkerUpdateChecks().
    showToast(changeToastMessage('Applied', threadId, desc), 'success', {
      key: applyKey,
      onClick: () => focusThread(threadId),
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
      // Don't light the badge here. It now shares the toast's single honest source
      // of truth — the build-id check (syncClientUpdateFromBuild) — so badge and
      // toast can't disagree or appear out of order. At ChangeApplied time the
      // rebuilt bundle isn't served yet, so an eager badge would lead the real
      // update. Instead nudge the SW to pick up the rebuilt /sw.js over the next
      // few seconds; its activation re-runs the build-id check, which lights BOTH
      // badge and toast together once the new build is genuinely served. For a
      // frontend-only Apply the engine re-snapshots its served dist in-process
      // (engine::frontend_refresh), so the served sw.js advances within a few
      // seconds without a respawn — this nudge is what surfaces it.
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

  // The "merge conflict — resolving automatically" banner is a sticky warning
  // (no auto-dismiss). Once the conflict reaches a terminal state it must not
  // keep claiming it's still resolving — transition it in place. Guarded on the
  // toast already existing: showToast(key) would otherwise CREATE a banner, so a
  // plain (non-conflict) apply never spawns a spurious "resolved" toast.
  // ChangeApplied → update to "resolved" (the visible answer to "this should
  // update to resolved when resolved"); ChangeApplyFailed / ChangeDiscarded just
  // dismiss it — the terminal toast above already carries that outcome.
  if ((event.type === 'ChangeApplied' || event.type === 'ChangeApplyFailed' || event.type === 'ChangeDiscarded')
      && event.change_id) {
    const conflictKey = mergeConflictToastKey(threadId, event.change_id);
    if (toasts.value.some((t) => t.key === conflictKey)) {
      if (event.type === 'ChangeApplied') {
        showToast(`Merge conflict in ${formatThreadLabel(threadId)} — resolved.`, 'success', {
          key: conflictKey,
          onClick: () => focusThread(threadId),
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
      key: mergeConflictToastKey(threadId, event.change_id),
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

    case 'NativePushDismissRequested':
      // A notification was read (here or on another device); the engine asks a
      // connected Tauri desktop app to REMOVE its already-delivered native
      // banner(s). Browser / PWA pages ignore it (handleNativePushDismiss gates
      // on isTauri — the open web can't silently remove a Web Push banner). See
      // system-knowhow/notifications.md §4.
      handleNativePushDismiss(data as unknown as NativePushDismissRequestedPayload);
      break;

    case 'PreferencesChanged':
      // A peer device may have just dismissed the client-refresh toast globally
      // (the `client_refresh_dismissed_build` preference — hooks/sw-update.ts), so
      // reload preferences and THEN re-derive the client-update surface to hide the
      // toast on this device too. Ordered: syncClientUpdateFromBuild reads the
      // reloaded `preferences` signal via `wasSwUpdateDismissed`, so it must run
      // after loadPreferences resolves (else it reads the stale value and the
      // reload wouldn't re-trigger it). Idempotent + self-correcting (re-derives
      // badge + toast from staleness). The engine-switch toast needs no equivalent
      // here — its 4s version-status poll (engine-update.ts) already hides itself
      // once `wasSwitchDismissed` reads true from the reloaded preferences.
      // loadPreferences sets `preferences` to `failed` via toFailed on error — no
      // extra surface needed here.
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
    // `set_language` / `set_timezone` (chat-agent tools) write the preference
    // and emit LanguageSet / TimezoneSet but NOT PreferencesChanged, so without
    // these arms the cached `preferences` (and thus the locale/timezone shown in
    // the UI) would stay stale until reload. loadPreferences re-reads the full
    // map, covering both.
    case 'LanguageSet':
    case 'TimezoneSet':
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

    case 'FrontendUpdateDeferred':
      // Dev-only transient signal: a frontend-only Apply couldn't advance the
      // served client in-process because an engine version change is pending
      // (engine::frontend_refresh INV-A). The change ships on the next Switch;
      // surface a keyed hint so it reads as queued, not ignored.
      handleFrontendUpdateDeferred(data as unknown as FrontendUpdateDeferredPayload);
      break;

    case 'FrontendUpdateStranded':
      // Dev-only transient signal: a frontend-only Apply rebuilt, but the engine
      // serves a dist/ that nothing republishes into, so the change can never
      // reach this client and no Switch will deliver it
      // (engine::frontend_refresh's rebuild wait timed out). Warn, with the
      // served path, instead of the pre-2026-07-26 silence.
      handleFrontendUpdateStranded(data as unknown as FrontendUpdateStrandedPayload);
      break;

    case 'ServedFrontendAdvanced':
      // Dev-only transient signal: THIS engine advanced its served-frontend
      // snapshot to the checkout-shared dist/ after a PEER workspace's
      // frontend-only Apply (engine::frontend_refresh::sync_served_frontend_if_safe).
      // Re-run the honest build-id check so the Refresh badge/toast surface without
      // a manual restart — idempotent + self-correcting, so no payload needed.
      void syncClientUpdateFromBuild();
      break;

    case 'EngineBuildStateChanged':
      // Dev-only transient POKE: the engine's background rebuild changed state
      // (building → ready/failed). Re-run the authoritative version-status read so
      // the building spinner / Switch badge track a real build over SSE instead of
      // only on the throttled 4s poll (iOS suspends it on a backgrounded PWA).
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
      changes.value = { status: 'loaded', data: pending };
      appliedChanges.value = { status: 'loaded', data: applied };
      // `changesHasMore` tracks whether more APPLIED changes are pageable, and
      // the ChangesUpdated payload carries no `has_more_applied` (its
      // `total_pending` is literally `pending.len()`, so a pending-count
      // comparison is always false). Deriving it here silently killed the
      // applied-list infinite scroll; leave the flag to refreshChangesState and
      // loadMoreChanges, which read the real field.
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
    //   1. origin_device_id — server rebroadcasts to all devices including
    //      the originator; ignore our own echo. NOTE: this only suppresses a
    //      PRESENT origin equal to self. A broadcast with an ABSENT origin (a
    //      PUT that fired before the device-id header was available) bypasses
    //      this check; for the dangerous case — an empty payload that would
    //      clear a locally-typed draft — applyRemoteCompose's own guard is the
    //      backstop (see docs/plans/2026-06-28-drafts-sse-empty-clear-guard.md).
    //      A non-empty absent-origin update still applies (it carries content),
    //      which is why this check is not widened to break on absent origin.
    //   2. pendingComposePuts — a debounced PUT may already be in flight
    //      with newer text; the SSE event for our previous PUT could clobber
    //      it on arrival otherwise.
    //   3. focused-textarea — if the user is mid-keystroke on this thread's
    //      input, dropping a peer's NON-empty update is safer than moving the
    //      cursor / blanking what they just typed. It must NOT drop a peer's
    //      EMPTY clear, though: that's the "follow-up sent/discarded elsewhere"
    //      signal, and gating it on focus left the peer's draft preserved in a
    //      focused-but-untyped textarea. applyRemoteCompose's own
    //      hasUnsentLocalDraft guard still protects unsent work THIS device
    //      authored.
    case 'ThreadComposeChanged': {
      const id = data.id as string;
      // Recorded BEFORE either guard below. The *compose epoch* is a fact about
      // what the engine holds, not draft content, so neither "this is our own
      // echo" nor "we have a write in flight" is a reason to ignore it. The
      // device with a write in flight is in fact the one that needs it most:
      // its next write is fenced against this value, and learning it here is
      // what saves a 412 round trip after every send.
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
        // Per-draft dropdown selection (DB-backed); hydrated into composeSelections
        // so a peer's dropdown change syncs. Absent (`skip_serializing_if` when the
        // DB has no stored selection) → undefined → setComposeSelectionFromServer
        // clears any stale local entry (the DB is authoritative). An in-flight local
        // pick is already protected by the pendingComposePuts guard above.
        selection: (data.selection as ComposeSelectionOverride | null | undefined),
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

/** Tool names whose `ToolResult` means `data/` may have changed, so the Files
 *  list and any open file preview must re-read.
 *
 *  The five file tools are the obvious members. `bash_output` is here for a
 *  different reason: a background task (`run_bash_background` /
 *  `run_python_background`) writes to `data/` UNSTAGED by design, so a
 *  long-running job can let apps see partial output as it lands (see
 *  `engine/tools/python.rs`). No `Artifact*` or `DataFile*` event is emitted for
 *  those writes, which leaves a drain as the only signal the frontend gets that
 *  output has appeared.
 *
 *  Plain `run_bash` is deliberately absent. Its tool description forbids writing
 *  to `data/` (that is `run_python`'s job), and every entry here costs a full
 *  `data/` walk server-side via `list_artifacts`, which is not worth paying
 *  after each curl, ls and git status. A bash write to `data/` is tool misuse;
 *  the header Refresh button covers it. */
const ARTIFACT_REFRESHING_TOOLS = [
  'write_file', 'edit_file', 'copy_file', 'delete_file', 'import_file', 'bash_output',
];

/** Handle transient ThreadEvent types that trigger side effects (modals, refreshes).
 *
 *  `sourceThreadId` is the thread the event was emitted on — used to scope
 *  `NavigationRequested` so a navigate from a background/sibling thread can't
 *  hijack the page the user is actually viewing. */
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
          await initPushSubscription();
          await toggleDevicePush(getDeviceId(), true);
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
    // preview, so this arm is what makes an agent edit show up without the user
    // touching anything. (loadArtifacts sets `artifacts` to `failed` via
    // toFailed on error.)
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
      // Scope: a navigate acts on this page directly only when it originates
      // from the thread the user is currently viewing (LLM navigate_ui in the
      // focused thread) OR from an app iframe — the SDK `lucidos.ui.navigate`
      // path emits on the nil thread (api/sdk.rs), which is user-initiated and
      // not thread-bound, so it always applies.
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
      // `handleThreadEvent`'s bottom bump only covers the thread the event
      // ARRIVED on, and the transient-event path above it returns before
      // reaching that bump at all, so fire the per-thread bumps here for both
      // threads whose pendingUserMessages just changed. Without these, the
      // originating thread keeps painting the just-moved 'Requesting...' row,
      // and the new CC thread doesn't render the seeded pendingUserMessages
      // until its first real event lands.
      if (movedMessages && currentThreadId) bumpThreadEvents(currentThreadId);
      bumpThreadEvents(e.cc_thread_id);
      break;
    }
  }
}

export { handleNavigationRequest } from './navigation-request';
