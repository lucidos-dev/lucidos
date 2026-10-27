import { API } from '../../api/client';
import { isChosenByPreference, isChosenForThread } from '../windowPeers';
import { forgetThreadWidgets, onWidgetEvent } from './widgets';
import type { ApplyEstimates, Change, LandingLane, MainCheck } from '../../api/client';
import { EVENT_STREAM_RETRY_MS, eventStreamTargets, openEventStream, type EventStreamTargets, type StreamDrop } from '@lucidos/event-stream';
import { postClientLog } from '../../utils/clientLog';
import { getEventStream, setEventStream } from './event-stream';
import { fanOutEventFrame, fanOutEventStreamStatus } from './app-bridge';
import { threadMap, focusedThreadId, changes, appliedChanges, setAsideChanges, applyingChangeIds, landingLane, applyAllInProgress, applyAllBatch, applyAllCanceling, APPLY_ALL_SUMMARY_TOAST_KEY, applyEstimates, applyPhases, standingApplyThreadIds, generatedTitleIds, codingAgentSessionVersion, setFocusedThread, removingQueuedMessageIds, queuedMessageRemovalKey, UNREAD_HOLD_EVENT_TYPES } from '../store';
import { backgroundModelsVersion, chatModels, findChangeById, memoryRebuildProgress, summaryTreesVersion, backupProgress, backupStatusVersion, backupPreferencesVersion, backupRecipientVersion, intelligenceLadderVersion, recommendedCleanupProgress, diskUsageVersion, promptFootprintVersion, responseStylesVersion, recoveryProgress, showConfirm, showToast, dismissToast, repoSource, TOAST_AUTO_DISMISS_MS } from '../store';
import { describeRecommendedCleanupOutcome } from '../../utils/recommendedCleanup';
import { isFormRequest, isWidgetEvent } from '../thread-events/thread-event-types';
import { rereadLoadedDashboards } from './dashboards';
import { liftRowIntent, overlayRowIntent } from '../rowIntents';
import { handleEvent, isChannelDefiningEvent, isSummaryCurrent, makeOptimisticThreadState, PENDING_TITLE_PLACEHOLDER, type ThreadAggregate, type ThreadMeta, type ThreadEvent, type TransientEvent } from '../thread-events';
import { bumpThreadEvents } from '../threadActivity';
import { settleDeliveredUnsentMessage } from './sendSettlement';
import { noteImageLanded } from '../landedImages';
import type { ThreadChannel } from '../store';
import { handleNotificationSSE, loadUnreadNotifications } from './notifications';
import { dropDeletedThreads } from './threads-drop';
import { loadThreadQueue } from './threadQueue';
import { handlePresenceCheck, type PresenceCheckPayload } from './presence-pong';
import { noteArrivalWhileAway, noteArrivalWhilePresent } from './seen-target';
import { dropAllNotificationToasts, dropNotificationToast } from './in-app-notification-toast';
import {
  handleNativePushRequested,
  type NativePushRequestedPayload,
  handleNativePushDismiss,
  type NativePushDismissRequestedPayload,
} from './native-push';
import { addRestartGroup, applyChangeSummarized, refreshChangesState, STANDING_APPLY_CANCELED } from './chat-changes';
import {
  handleFrontendUpdateDeferred,
  handleFrontendUpdateStranded,
  handleEngineBuildStateChanged,
  handleFrontendRefreshStateChanged,
  type FrontendUpdateDeferredPayload,
  type FrontendUpdateStrandedPayload,
} from './engine-update';
import {
  handleFrontendPreviewStarted,
  handleFrontendPreviewStopped,
} from './frontend-preview';
import { changeToastMessage } from './changeToast';
import { changeHeadline, changeNamingFromEvents } from '../changeHeadline';
import { batchSummary, clearApplyPhase, isBatchMember, openApplyPhase, setApplyPhase } from './applyProgress';
import { handleLaneFrame, laneWithChangeQueued, laneWithoutWaiting, setLandingLaneFromServer } from './landingLane';
import { handleMainCheckFrame, setMainCheckFromServer } from './mainCheck';
import { repairGapState } from '../eventGap';
import { syncClientUpdateFromBuild } from './client-update';
import { loadPreferences, refreshActiveTheme } from './preferences';
import { loadChatModels } from './models';
import { loadReleaseNotices } from './releaseNotices';
import { loadArtifacts, refreshArtifacts } from './artifacts';
import { refreshAppUI, captureAppUI } from './apps';
import { clearWipIfMatches } from './wipPreview';
import { closeResolvedFormRequest, openFormRequest, syncPendingFormRequests } from './form-requests';
import { setDevicePushEnabled } from './push';
import { getDeviceId } from './devices';
import { focusThread } from './threads';
import { refreshRepoView } from './repositories';
import {
  processSSEForReferences,
  refreshLlmConfigured,
  PROVIDER_PREFERENCE_KEYS,
} from './entityReferences';
import { refreshThreadEvents, loadThreadEvents, forgetThreadEventsFailures, markLoadedThreadsStale } from './thread-loading';
import { refreshThreadList } from './thread-list-refresh';
import { acceptComposeReport, parseComposeReport, type ComposeReportWire } from '../composeSync';
import { clearDraft, setDraft } from '../composeDrafts';
import { removeThreadNavEntries } from './thread-navigation';
import { isComposeFocusedHere } from '../../components/chat/promptFocus';
import { formatBytes } from '../../utils/formatBytes';
import { errorDetail } from '../../utils/errorDetail';
import { routeThreadNavigation } from './navigation-request';
import { isForThisDevice, type DeviceScopeActor } from './device-scope';
import { openBackupSettings } from './menu';
import { applyEmbeddingModelStatus } from './backgroundActivity';
import { CHAT_DEFAULT_PREFS, CHAT_LADDER_PREF, LADDER_PREFS, LONG_PROMPT_SHARE_PREF } from '../intelligenceLadderPrefs';
import {
  applyTreeBackfillFrame,
  applyTreeBackfillProgress,
  forgetTreeBackfillOrder,
  readyAfterCompleted,
} from './treeBackfill';
import type { BackfillProgress, EmbeddingModelStatus } from '../../api/types';

/** Keyed so a second failure replaces the first instead of stacking, and so the
 *  tap can dismiss the toast it just acted on. */
const BACKUP_FAILED_TOAST_KEY = 'backup-failed';

/** Keyed for the same reason: the engine re-announces its refused `apis.json`
 *  entries on every boot, and a reconnecting client must not stack them. */
const PROXY_CONFIG_REJECTED_TOAST_KEY = 'proxy-config-rejected';

let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let repoChangesDebounce: ReturnType<typeof setTimeout> | null = null;

/** Where the shell reaches the engine's event surface. The suffixes live in the
 *  SDK beside the transports, so the shell and an app cannot name them
 *  differently. */
function hostTargets(): EventStreamTargets {
  return eventStreamTargets(API);
}

function markEventStreamStatus(status: 'connecting' | 'connected' | 'disconnected'): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.lucidosEventStream = status;
}

/** Set when `onerror` fires, consumed by the next `onopen`, so the resync runs
 *  only on RECONNECT. On the initial connect `loadAllThreads()` is already
 *  driving state via startup.ts. */
let needsResyncOnOpen = false;

/** In-flight resync coalescer. Multiple Lagged events, or back-to-back
 *  reconnects, collapse into one network round-trip. */
let resyncInFlight: Promise<void> | null = null;

/** The preference keys the Backup page renders, mirroring the `PREF_BACKUP_*`
 *  constants in the engine's `core/backup/mod.rs`. Only a `PreferencesChanged`
 *  carrying one of them makes that page re-read. Every other key must leave its
 *  endpoints alone. */
const BACKUP_PREFERENCE_KEYS = new Set([
  'backup_provider',
  'backup_schedule',
  'backup_retention',
]);

/** The preferences the served intelligence ladder reads. A `PreferencesChanged`
 *  for one of them makes Settings → Coding agents re-read the ladder. */
const INTELLIGENCE_LADDER_PREFERENCE_KEYS = new Set([
  ...Object.values(LADDER_PREFS).flatMap((p) => [p.ladder, p.model, p.effort]),
  CHAT_LADDER_PREF,
  ...CHAT_DEFAULT_PREFS,
  LONG_PROMPT_SHARE_PREF,
]);

// ---------------------------------------------------------------------------
// Batched threadMap updates — coalesce rapid SSE events into one signal write
// per animation frame. Without this, every SSE event triggers O(N*M)
// recomputation in computed signals (blockedThreadCount iterates ALL threads
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

/** The headline a change toast names the change by (`changeHeadline`).
 *
 *  The loaded row first, since it carries the stored summary. Else the thread's
 *  own events: the latest proposal, and the summary of that commit list. */
function findChangeHeadline(threadId: string, changeId: string): string | undefined {
  const naming = findChangeById(changeId)
    ?? changeNamingFromEvents(threadMap.value.get(threadId)?.events.values() ?? [], changeId);
  return naming ? changeHeadline(naming) : undefined;
}

/** Route one frame's `data` payload into the store.
 *
 *  Module-private, and the one sink both transports reach: `onFrame` below
 *  hands it a direct frame and a worker-relayed one alike, which is what makes
 *  the two indistinguishable from here down. */
function handleHostFrame(data: string): void {
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(data);
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
  const payload = (parsed.data ?? {}) as Record<string, unknown>;

  if (type === 'ThreadEvent') {
    handleThreadEvent(payload);
  } else {
    handleGlobalEvent(type, payload);
  }

  // Entity sync: recents, nav stack, pinned apps, store refresh.
  processSSEForReferences(type, payload);
}

export function connectThreadEvents(): void {
  if (getEventStream()) return;

  const gen = ++sseGeneration;
  markEventStreamStatus('connecting');

  const handlers = {
    onFrame: (data: string) => {
      // Stale handler: a newer connection replaced this one. The old transport
      // was closed, but its queued message handler can still fire.
      if (gen !== sseGeneration) return;
      handleHostFrame(data);
      // An isolated app frame can open no stream of its own, so it reads this
      // one. Verbatim, and after the shell's own handling, so a slow app never
      // delays the shell's repaint.
      fanOutEventFrame(data);
    },

    onOpen: () => {
      if (gen !== sseGeneration) return;
      markEventStreamStatus('connected');
      fanOutEventStreamStatus('open');
      // The first open reads what a frame emitted while no stream was up
      // would have refreshed. A reopen after an error reads it through the gap
      // reconcile, so it never runs both. On the first open startup.ts already
      // loads thread state, and a resync there would double-fetch every page
      // load.
      const reopenedAfterError = needsResyncOnOpen;
      needsResyncOnOpen = false;
      if (reopenedAfterError) reconcileAfterEventGap();
      else readStateMissedBeforeOpen();
    },

    onError: (drop: StreamDrop) => {
      // Stale handler: disconnectThreadEvents() already closed this transport
      // and connectThreadEvents() created a replacement. Without this guard the
      // old handler closes the NEW connection, via the module-scoped
      // `eventStream`, costing a 3s SSE gap on every iOS Safari PWA resume.
      if (gen !== sseGeneration) return;

      if (drop.reason === 'stalled') {
        postClientLog('event-stream', 'stalled', { silentMs: drop.silentMs, transport: drop.transport });
      }

      // Mark for resync. Events emitted during the gap never reach this tab, so
      // the next successful connect must refetch persisted state.
      needsResyncOnOpen = true;
      // The engine may restart in the gap, and its backfill frames then count
      // from one again. A snapshot read meanwhile sets the new order.
      forgetTreeBackfillOrder();
      markEventStreamStatus('disconnected');
      fanOutEventStreamStatus('error');

      // A transport that retries for itself keeps its connection: the shared
      // worker owns the one upstream, and dropping our port would take that
      // stream down for every other document. Its next `open` lands here and
      // runs the resync armed above.
      if (getEventStream()?.ownsReconnect) return;

      getEventStream()?.close();
      setEventStream(null);
      if (reconnectTimer) clearTimeout(reconnectTimer);
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null;
        connectThreadEvents();
      }, EVENT_STREAM_RETRY_MS);
    },
  };

  // The shell answers a PresenceCheck, so it registers as a ponger. On a shared
  // connection the worker merges its ports' answers into the one pong the engine
  // waits for.
  setEventStream(openEventStream(hostTargets(), handlers, { pongs: true }));
}

/** Make sure a stream is up after a resume, without cutting a healthy one.
 *
 *  A resume runs on every window focus, and a click that raises the window
 *  fires focus beside the action it caused. A frame the engine emits in a
 *  teardown gap is never replayed, so a navigate requested by that click would
 *  be lost. Only a missing or stale stream is replaced. A shared stream the
 *  worker finds silent reports through `onError`, then `onOpen`, and
 *  `reconcileAfterEventGap` repairs it there. */
export function reviveThreadEvents(): void {
  const stream = getEventStream();
  if (stream && stream.revalidate() !== 'stale') return;
  disconnectThreadEvents();
  connectThreadEvents();
}

/** The one repair after an event gap: a reopen after an error, or `Lagged`.
 *  Events emitted in the gap reach nobody. Every signal only a frame can move
 *  is repaired here (`repairGapState`), and every read a frame would have
 *  triggered runs again. Nothing else repairs a gap, so neither path can lack a
 *  repair the other has.
 *
 *  `ServedFrontendAdvanced` is transient too. A swap during the gap announced a
 *  newer client to nobody, so the build-id check runs again. Each read below
 *  coalesces and reports its own failures, so `void` only acknowledges that the
 *  promise is not needed back.
 *
 *  A wake that replaces a stale stream opens it as a first open, and one that
 *  keeps a live stream opens nothing. `runResumeSync` therefore repairs the
 *  signals itself (`repairGapState`). */
function reconcileAfterEventGap(): void {
  repairGapState();
  void resyncLoadedThreads();
  resyncChangesAndQueue();
  void syncClientUpdateFromBuild();
  readStateMissedBeforeOpen();
}

/** The reads whose frames reach this page only if a stream was up: a form
 *  request, the Files list, the unread notifications and the preferences, so a
 *  theme picked in that window still paints. The page's first reads can finish
 *  before the first open, and a change in between was announced to nobody.
 *  None can land stale: an open during a listing asks for one more, and the
 *  newest unread read wins. Each owns its own failure reporting. */
function readStateMissedBeforeOpen(): void {
  void syncPendingFormRequests();
  refreshArtifacts();
  void loadUnreadNotifications();
  void loadPreferences();
}

/** Refetch thread metadata for every thread, and missed events for the focused
 *  one. Called by `reconcileAfterEventGap` when SSE drops and reconnects, or
 *  when the backend signals `Lagged` (its broadcast subscriber fell behind the
 *  buffer and dropped events).
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
      // One events request, for the thread on screen. The metadata read above is what
      // repairs the drawer, and `refreshStaleThreadEvents` consumes the rest of
      // the marks on focus.
      //
      // After the metadata read, deliberately, so the refresh sees the
      // authoritative state. Coalesced against a concurrent wake resync, and
      // `refreshThreadEvents` surfaces its own failures.
      const focused = focusedThreadId.value;
      const refreshFocused = !!focused && !!threadMap.value.get(focused)?.eventsLoaded;
      if (refreshFocused) {
        await refreshThreadEvents(focused, { coalesce: true });
        // A widget event missed in the gap changed the thread's widgets too.
        onWidgetEvent(focused);
      }
      // The dashboards decide Add to dashboard in every widget menu, so a
      // dashboard event missed in the gap matters there too.
      rereadLoadedDashboards();
    } finally {
      resyncInFlight = null;
    }
  })();
  return resyncInFlight;
}

/** Re-read the Changes and Thread Queue state after a stream gap. Neither
 *  `ChangesUpdated`, `ApplyAllBatchCompleted` nor a queue event is replayed.
 *  Without this, a batch that finished in the gap reads "in progress" until
 *  the next wake or reload. A wake runs `runResumeSync`, which re-reads both
 *  itself. Both reads report their own failures and never reject. */
function resyncChangesAndQueue(): void {
  void refreshChangesState();
  void loadThreadQueue();
}

export function disconnectThreadEvents(): void {
  // Bump generation BEFORE closing, so any onerror handler queued by the
  // close() call sees a stale generation and bails out.
  sseGeneration++;
  // An explicit disconnect means the caller is taking ownership of state
  // recovery. Do not let the next onopen also resync, or every real reconnect
  // doubles the per-thread refreshThreadEvents fan-out.
  needsResyncOnOpen = false;
  // A resume sync disconnects here, often across an engine restart.
  forgetTreeBackfillOrder();
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  markEventStreamStatus('disconnected');
  // The closed transport's own error handler is now stale and stays silent, so
  // app frames hear the drop here. Their next `open` is the reconnect.
  fanOutEventStreamStatus('error');
  getEventStream()?.close();
  setEventStream(null);
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
  if (isWidgetEvent(event)) onWidgetEvent(threadId);
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
  // streaming token would re-execute blockedThreadCount and every visible
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
    // A persisted event with no aggregate has no DB row either, so it produces
    // the same phantom. The engine builds the aggregate from `thread_summaries`
    // inside the emitting transaction, so its absence means that row is gone.
    // The warning above already named it. A skeleton built from nothing put a
    // titleless row in the drawer, which a reload then swept away.
    if (!aggregate) return;
    // Title, channel, initiator and createdAt come from the aggregate, never
    // from this event. The event need not be the thread's first: a background
    // WorktreeCleaned reaches archived threads the drawer never loaded.
    const isThreadStarted = event.type === 'ThreadStarted';
    const startedMode = isThreadStarted ? ((event as Record<string, unknown>).mode as string | undefined) : undefined;
    map.set(threadId, makeOptimisticThreadState({
      id: threadId,
      title: aggregate.title || PENDING_TITLE_PLACEHOLDER,
      channel: aggregate.channel as ThreadMeta['channel'],
      initiator: aggregate.initiator,
      eventsLoaded: isThreadStarted, // composing has no events to load
      timestamp: aggregate.createdAt,
      ...(isThreadStarted ? {
        state: 'composing' as const,
        status: 'idle' as const,
      } : {}),
    }));
    // New thread row inserted into the map — every subscriber needs to learn
    // about it (drawer rows, blockedThreadCount, focused-thread router).
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

  // seq from SSE: present (number > 0) for persisted events, null for transient
  const aggregateWasCurrent = aggregate !== undefined && aggregate !== null
    && isSummaryCurrent(thread.meta, aggregate.summaryVersion);
  if (aggregateWasCurrent) liftRowIntent(thread.meta);
  const handled = handleEvent(map, threadId, seq, event, created, eventId, aggregate);
  if (handled.metaChanged) metaChanged = true;
  if (handled.retiredUnsentEventId) settleDeliveredUnsentMessage(handled.retiredUnsentEventId);
  if (event.type === 'ImageUploaded') noteImageLanded(threadId, event.hash);
  if (event.type === 'QueuedMessageRemoved') {
    const key = queuedMessageRemovalKey(threadId, event.removed_message_id);
    if (removingQueuedMessageIds.value.has(key)) {
      const next = new Set(removingQueuedMessageIds.value);
      next.delete(key);
      removingQueuedMessageIds.value = next;
    }
  }
  // The Not ready strip and the Blocked badge read the hold from events.
  if (UNREAD_HOLD_EVENT_TYPES.has(event.type)) metaChanged = true;

  // A row the user changed optimistically keeps that change over this
  // frame's aggregate until the row reaches the version the write returned
  // (ADR 0437). A cascade archive's `stop_agent` emits pre-archive frames,
  // and they can land after the POST returned.
  if (aggregateWasCurrent && overlayRowIntent(thread.meta)) metaChanged = true;

  // The draft is not touched here: the send consumed the compose slot at
  // accept, and the engine reported what that left (ADR 0450).
  if (event.type === 'MessageReceived') {
    if (thread.meta.state !== 'active') { thread.meta.state = 'active'; metaChanged = true; }
  }
  if (event.type === 'ThreadDiscarded') {
    if (thread.meta.state !== 'discarded') { thread.meta.state = 'discarded'; metaChanged = true; }
    if (!discardedHere(event)) {
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

  // Toast for change state transitions. An apply's progress is told only in
  // the Lucidos menu, so its one toast, keyed `applying-<thread>`, is the
  // result. A batch member's success is the batch summary's to report, so it
  // raises only its failure here.
  if (event.type === 'ChangeApplied') {
    const lastPhase = clearApplyPhase(threadId);
    const inBatch = isBatchMember(applyAllBatch.value, event.change_id);
    resolveBatchMember(event.change_id);
    const desc = event.change_id ? findChangeHeadline(threadId, event.change_id) : undefined;
    const requiresRestart = !!event.requires_restart;
    const applyKey = `applying-${threadId}`;
    // No Refresh button and no update badge here. At ChangeApplied time the
    // rebuilt frontend is not served yet, the build-watch still running `vite
    // build`, so a Refresh now would reload the OLD build. The genuine
    // affordance is the New-version toast (store/actions/client-update.ts).
    // The engine's `ServedFrontendAdvanced` fires it the moment the rebuilt
    // client is served, and a mixed change reaches it through the Switch.
    // The Applied toast lands where the apply's thread link did: at the event
    // that started its last phase.
    if (inBatch) {
      dismissToast(applyKey);
    } else {
      const changeId = event.change_id;
      showToast(changeToastMessage('Applied', threadId, desc), 'success', {
        key: applyKey,
        onClick: () => openApplyPhase(threadId, changeId, lastPhase),
        autoDismissMs: TOAST_AUTO_DISMISS_MS,
      });
    }
    // Record the restart state immediately from the thread event, rather than
    // waiting for the separate ChangesUpdated system event. If ChangesUpdated is
    // missed (SSE drop, Vite reload race), this is what lights the badge.
    if (requiresRestart) {
      const commits = event.commits ?? [];
      const threadTitle = event.thread_title ?? threadMap.value.get(threadId)?.meta.title ?? 'Untitled thread';
      addRestartGroup({ threadId, threadTitle, commits });
    }
  } else if (event.type === 'ChangeDiscarded') {
    clearApplyPhase(threadId);
    const desc = event.change_id ? findChangeHeadline(threadId, event.change_id) : undefined;
    showToast(changeToastMessage('Discarded', threadId, desc), 'success', {
      key: `discarding-${threadId}`,
      onClick: () => focusThread(threadId),
      autoDismissMs: TOAST_AUTO_DISMISS_MS,
    });
  } else if (event.type === 'ChangeReverted') {
    const desc = event.change_id ? findChangeHeadline(threadId, event.change_id) : undefined;
    showToast(changeToastMessage('Reverted', threadId, desc), 'success');
  } else if (event.type === 'ChangeApplyFailed') {
    clearApplyPhase(threadId);
    resolveBatchMember(event.change_id);
    const error = event.error ?? 'Unknown error';
    showToast(changeToastMessage('Failed to apply', threadId, error), 'error', { key: `applying-${threadId}`, onClick: () => focusThread(threadId) });
  } else if (event.type === 'ChangeUnqueued') {
    // The change left the landing lane without landing, so its apply ended.
    // No toast: LaneBatchFailed reports a failed batch, ChangeApplyFailed a
    // failed landing, and every other reason is the user's own action.
    clearApplyPhase(threadId);
    resolveBatchMember(event.change_id);
  }

  // The lane and the pending list move on the change's own event. Their
  // snapshot trails it by a merge prediction per change, and the row would
  // read Apply until it arrived. The next snapshot replaces both whole.
  if (event.type === 'ChangeQueued' && event.change_id) {
    landingLane.value = laneWithChangeQueued(landingLane.value, event.change_id);
  } else if ((event.type === 'ChangeUnqueued' || event.type === 'ChangeApplied') && event.change_id) {
    landingLane.value = laneWithoutWaiting(landingLane.value, event.change_id);
  }
  if (event.type === 'ChangeApplied' && event.change_id) {
    const landed = event.change_id;
    const pending = changes.value;
    if (pending.status === 'loaded' && pending.data.some((c) => c.id === landed)) {
      changes.value = { status: 'loaded', data: pending.data.filter((c) => c.id !== landed) };
    }
  }

  // After apply/discard/revert, reveal the app header on mobile so the result
  // is readable with full navigation visible. The transcript is NOT moved: the
  // resolution card lands below whatever the reader is looking at, and the
  // chevron is how they go to it.
  if (event.type === 'ChangeApplied' || event.type === 'ChangeDiscarded' || event.type === 'ChangeReverted') {
    if (threadId === focusedThreadId.value) {
      document.dispatchEvent(new Event('reveal-mobile-bars'));
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

  // Clear applyingChangeIds when a change's apply ends.
  if (event.type === 'ChangeApplied' || event.type === 'ChangeApplyFailed' || event.type === 'ChangeUnqueued') {
    if (event.change_id && applyingChangeIds.value.has(event.change_id)) {
      const next = new Set(applyingChangeIds.value);
      next.delete(event.change_id);
      applyingChangeIds.value = next;
    }
  }

  if (event.type === 'ChangeSummarized' && event.change_id && event.summary) {
    applyChangeSummarized(event.change_id, event.summary, event.description ?? '');
  }

  // Track change_id as "applying" when merge conflict resolution starts.
  if (event.type === 'MergeConflictDetected' && event.change_id
      && !applyingChangeIds.value.has(event.change_id)) {
    applyingChangeIds.value = new Set([...applyingChangeIds.value, event.change_id]);
  }

  // The apply's phase. Every engine path that hardens or resolves a conflict
  // emits one of these two, so the activity group follows them whoever started
  // the apply. Its thread link lands on the event that started the phase. A
  // re-propose or a hardened stamp means the apply is back to merging.
  if (event.type === 'MergeConflictDetected' || event.type === 'MissingHardeningDetected') {
    const phase = event.type === 'MergeConflictDetected' ? 'resolving-conflict' : 'hardening';
    setApplyPhase(threadId, { phase, eventId: eventId ?? null, startedAt: created ?? null });
  } else if ((event.type === 'ChangeHardened' || event.type === 'ChangeProposed')
      && applyPhases.value.has(threadId)) {
    // Only an apply in flight goes back to merging. A thread often hardens
    // long before anyone applies it. A phase set then would list a merge in
    // the menu that never started.
    setApplyPhase(threadId, { phase: 'merging', eventId: null, startedAt: null });
  }

  // Per-thread "events arrived" bell — fires for every event so subscribers
  // to this specific thread (focused ChatExchange / ThreadView /
  // activeStreamingBuffer) recompute. Streaming tokens land here exclusively
  // and don't reach the `threadMap` flush below.
  bumpThreadEvents(threadId);

  // Global `threadMap` flush ONLY when meta-shape actually changed. Skipping it
  // for streaming-only arrivals is the whole point. blockedThreadCount,
  // ThreadDrawer.ThreadList, every visible ChatExchange and every PromptInput
  // effect read `threadMap.value` in their subscribe path, so they would
  // otherwise re-execute per CC token.
  if (metaChanged) {
    scheduleThreadMapFlush();
  }
}

/** Count a batch member as resolved, so the Apply All row moves to the next one. */
function resolveBatchMember(changeId: string | undefined): void {
  const batch = applyAllBatch.value;
  if (!batch || !changeId || !isBatchMember(batch, changeId)
      || batch.resolvedChangeIds.includes(changeId)) return;
  applyAllBatch.value = { ...batch, resolvedChangeIds: [...batch.resolvedChangeIds, changeId] };
}

/** Un-arm a thread's flag once its standing apply has ended, however it ended. */
function forgetStandingApply(threadId: string | undefined): void {
  if (!threadId || !standingApplyThreadIds.value.has(threadId)) return;
  const next = new Set(standingApplyThreadIds.value);
  next.delete(threadId);
  standingApplyThreadIds.value = next;
}

export function handleGlobalEvent(type: string, data: Record<string, unknown>): void {
  switch (type) {
    case 'NotificationCreated':
      // Bell badge only: the toast rides PresenceCheck (§3, §4). The seen target hold goes first, ahead of the unread reload
      // whose arrival starts the seen dwell.
      if (typeof data.id === 'string') noteArrivalWhilePresent(data.id);
      handleNotificationSSE();
      break;

    case 'NotificationRead':
      // The authoritative half of the toast's lifetime. The unread-set watch
      // answers a read this page can see, on the tick the reader acts. This
      // answers the two it cannot. One is a read made on another device. The
      // other is a read landing before the reload that carries its row, where
      // the id was never in the set to leave it. The engine emits this only on
      // a real unread to read flip, so a redundant write cannot double it.
      if (typeof data.id === 'string') dropNotificationToast(data.id);
      handleNotificationSSE();
      break;

    case 'NotificationsAllRead':
      dropAllNotificationToasts();
      handleNotificationSSE();
      break;

    case 'ThreadsDeleted': {
      // The owner deleted a thread and its family, here or on another device.
      // The rows are gone on the engine, so this is not a hint: every client
      // holding them is now wrong about what exists.
      const ids = Array.isArray(data.thread_ids) ? (data.thread_ids as string[]) : [];
      dropDeletedThreads(ids);
      forgetThreadWidgets(ids);
      // Idempotent on the device that made the delete, which already dropped
      // them, and it is what corrects every other one.
      //
      // The refresh is what picks up a SURVIVING ancestor's repaired descendant
      // counts. The engine recomputes them after the commit, and no per-thread
      // event carries the new value. Without a re-read the parent keeps a stale
      // blocking count and hides its own Archive.
      void refreshThreadList();
      // The family's notifications went with it, so the bell is stale too, and
      // so is any queue entry bound to a member. Neither moves on an event of
      // its own here: the rows went with the family.
      handleNotificationSSE();
      void loadThreadQueue();
      // A delete purges the family's summary trees in the same transaction.
      summaryTreesVersion.value++;
      break;
    }

    case 'PresenceCheck': {
      // Engine asks every connected page what it does with a new notification,
      // so it can decide whether to fan out the OS push. The page acts first
      // (toast, auto-read, or the badge alone) and pongs what it did. A check
      // too stale to act on, or one left to the OS, means the reader was away.
      // So the seen target hold goes. See system-knowhow/notifications.md §3, §4.
      const payload = data as unknown as PresenceCheckPayload;
      const outcome = handlePresenceCheck(payload);
      if (outcome === null || outcome === 'not_shown') noteArrivalWhileAway(payload.notification_id);
      break;
    }

    case 'NativePushRequested':
      // Engine allowed the OS push, no page having handled the notification,
      // and asks a connected Tauri desktop app to render a NATIVE macOS banner.
      // The WKWebView cannot receive the web push. Browser and PWA pages ignore
      // it, the handler gating on isTauri. See system-knowhow/notifications.md §4.
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
      // The report carries the three footprint preferences in force, and the
      // response style it measures is a preference too.
      promptFootprintVersion.value++;
      // A peer device may have just dismissed the client-refresh toast globally
      // via the `client_refresh_dismissed_build` preference. So reload
      // preferences and THEN re-derive the client-update surface, to hide the
      // toast here too. Ordered, because syncClientUpdateFromBuild reads the
      // reloaded `preferences` signal through `wasSwUpdateDismissed`: it must
      // run after loadPreferences resolves, or it reads the stale value.
      // Idempotent and self-correcting. The engine-switch toast needs no
      // equivalent, its version-status poll hiding it once
      // `wasEngineVersionDismissed` reads true. loadPreferences sets `preferences` to `failed` on error.
      // A `theme` write repaints even when it names the theme already painted:
      // that is how an agent republishes a theme it edited in place.
      void loadPreferences().then(() => {
        if (data.key === 'theme') void refreshActiveTheme();
        return syncClientUpdateFromBuild();
      }).catch(() => { /* best-effort re-derive */ });
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
      if (INTELLIGENCE_LADDER_PREFERENCE_KEYS.has(String(data.key ?? ''))) {
        intelligenceLadderVersion.value++;
      }
      // Each served model row says whether the chat ladder lists it, so the
      // chat picker and Settings → Models re-read the registry.
      if (data.key === CHAT_LADDER_PREF && chatModels.value.status === 'loaded') {
        void loadChatModels();
      }
      // A provider switch (or the free tier, or the local base URL) rebuilds
      // the engine's active provider set in-process, exactly as a credential
      // change does. So it takes the same `/health` re-probe: without it the
      // model picker keeps offering a provider the engine has just dropped.
      if (PROVIDER_PREFERENCE_KEYS.has(String(data.key ?? ''))) {
        refreshLlmConfigured();
      }
      // The *style library* the editor renders is not the stored document: the
      // engine merges it with what it ships, and only the engine holds the
      // shipped half. So the cache reloaded above cannot answer, and Settings
      // has to re-read `/response-styles`. Keyed, because a theme change must
      // not spend a request on it.
      if (String(data.key ?? '') === 'response_styles') {
        responseStylesVersion.value++;
      }
      // A release notice can be conditional on a preference, such as the Tree
      // notice on `memory_module`. Only the engine knows which keys matter, so
      // every change re-reads the list.
      void loadReleaseNotices();
      break;
    // The `set_language` and `set_timezone` chat-agent tools write the
    // preference and emit LanguageSet or TimezoneSet, but NOT
    // PreferencesChanged. Without these arms the cached `preferences` would
    // stay stale until reload. loadPreferences re-reads the full map.
    case 'LanguageSet':
    case 'TimezoneSet':
      void loadPreferences();
      break;

    case 'ReleaseNoticeResolved':
      // Another device answered the notice this one may be showing. Re-read the
      // list rather than closing locally: the answer moves the workspace on to
      // the NEXT notice, and this page has to step with it. The cursor behind
      // it is a silent preference, so this event is the only signal.
      void loadReleaseNotices();
      break;

    case 'AppUiRefreshRequested':
      // Transient system event aggregated on `app`. The engine emits it after
      // every app coding-agent apply touching an iframe-bundled file. The SDK
      // iframe of `app_id` reloads to pick up the merged content. The
      // thread-scoped variant, from `refresh_app` and the end of a chat turn,
      // is handled in `handleTransientSideEffects`.
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
      // Dev-only transient signal: THIS engine swapped its served-frontend
      // snapshot to the rebuilt dist/, after a frontend-only Apply here or in a
      // peer workspace. Re-run the build-id check, so the Refresh badge and
      // toast surface at once. Idempotent and self-correcting, so no payload.
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

    case 'FrontendRefreshStateChanged':
      // Dev-only transient POKE: a frontend-only Apply's rebuild wait started
      // or ended. Re-read version-status, which drives "Building frontend".
      handleFrontendRefreshStateChanged();
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

    case 'TreeBackfillProgressed':
      applyTreeBackfillProgress(data.progress as BackfillProgress);
      break;

    case 'TreeBackfillCompleted':
      applyTreeBackfillFrame(readyAfterCompleted());
      summaryTreesVersion.value++;
      break;

    case 'TreeBackfillReset':
      // Only a switch to Classic clears the flag, so the module is off now. A
      // set flag means the trees were built, so the backfill had started.
      applyTreeBackfillFrame({ state: 'off', started: true });
      summaryTreesVersion.value++;
      break;

    case 'EmbeddingModelStatusChanged': {
      // Transient frame from the engine's background embedding-model loader:
      // download progress and every transition between downloading / loading /
      // ready / waiting / failed. Same shape as the
      // `/memory/embedding-model-status` snapshot startClient reads, so this is
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
      // one being merged right now. ChangeApplied, ChangeApplyFailed and
      // ChangeUnqueued clear each member id; ApplyAllBatchCompleted drops the
      // bulk flag.
      applyAllInProgress.value = true;
      const changeIds = (data.change_ids ?? []) as string[];
      applyAllBatch.value = { changeIds, resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] };
      applyAllCanceling.value = false;
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
      const total = applyAllBatch.value?.changeIds.length ?? applied.length + failed.length;
      applyAllBatch.value = null;
      applyAllCanceling.value = false;
      applyAllInProgress.value = false;
      if (total > 0) {
        const summary = batchSummary(applied.length, total);
        showToast(summary.message, summary.type, { key: APPLY_ALL_SUMMARY_TOAST_KEY, autoDismissMs: TOAST_AUTO_DISMISS_MS });
      }
      break;
    }

    case 'LaneBatchStarted':
    case 'LaneBatchLanded':
    case 'LaneBatchFailed':
      // The landing lane opened or closed its batch. What waits behind it
      // arrives with the `ChangesUpdated` the lane sends beside it.
      handleLaneFrame(type, data);
      break;

    case 'MainCheckStarted':
    case 'MainCheckRunLinked':
    case 'MainCheckPassed':
    case 'MainCheckFailed':
    case 'MainCheckAbandoned':
    case 'MainCheckRedClosed':
      // The main check started, linked its run or ended, or its red closed.
      // The snapshot arrives whole with the `ChangesUpdated` the engine sends
      // beside it.
      handleMainCheckFrame(type, data);
      break;

    case 'StandingApplyArmed': {
      // The owner armed a standing apply, possibly on another device. Every
      // change surface reads this set to render the armed face.
      const threadId = data.thread_id as string | undefined;
      if (threadId) {
        standingApplyThreadIds.value = new Set([...standingApplyThreadIds.value, threadId]);
      }
      break;
    }

    case 'StandingApplyFired': {
      // The arm ended by firing. ChangeApplied or ChangeApplyFailed reports how
      // the apply went, so this owes no toast of its own.
      forgetStandingApply(data.thread_id as string | undefined);
      break;
    }

    case 'StandingApplyDropped': {
      // The arm ended without applying: the owner took it back, or the thread
      // parked or failed. The last two owe a report, and `reason` is written
      // for the owner to read.
      const threadId = data.thread_id as string | undefined;
      forgetStandingApply(threadId);
      const reason = typeof data.reason === 'string' ? data.reason : '';
      // A cancel is the owner's own click, and the control already changed
      // face. Only a drop the engine decided is news.
      if (threadId && reason && reason !== STANDING_APPLY_CANCELED) {
        showToast(changeToastMessage('Standing apply dropped', threadId, reason), 'warning', {
          key: `standing-apply-${threadId}`,
          onClick: () => focusThread(threadId),
        });
      }
      break;
    }

    case 'ChangesUpdated': {
      const pending = (data.pending ?? []) as Change[];
      const applied = (data.applied ?? []) as Change[];
      changes.value = { status: 'loaded', data: pending };
      appliedChanges.value = { status: 'loaded', data: applied };
      setAsideChanges.value = { status: 'loaded', data: (data.set_aside ?? []) as Change[] };
      if (data.apply_estimates) applyEstimates.value = data.apply_estimates as ApplyEstimates;
      setLandingLaneFromServer(data.landing_lane as LandingLane | undefined);
      setMainCheckFromServer(data.main_check as MainCheck | undefined);
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
        if (currentRepo) void refreshRepoView(currentRepo);
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
      // Tapping the toast opens the Backup page: the health card, the error and
      // Grant access all live there. Without it the toast names a problem and
      // leaves the user to find the page.
      showToast(`Backup failed: ${String(data.error ?? 'Unknown error')}`, 'error', {
        key: BACKUP_FAILED_TOAST_KEY,
        onClick: () => {
          dismissToast(BACKUP_FAILED_TOAST_KEY);
          openBackupSettings();
        },
      });
      backupStatusVersion.value++;
      break;
    }

    case 'BackupRecipientSet':
      backupRecipientVersion.value++;
      break;

    case 'RecommendedCleanupStarted':
      recommendedCleanupProgress.value = { done: 0, total: 0 };
      break;

    // The bus orders a pass's frames, so no progress frame follows its end.
    case 'RecommendedCleanupProgress':
      recommendedCleanupProgress.value = {
        done: Number(data.done ?? 0),
        total: Number(data.total ?? 0),
      };
      break;

    case 'RecommendedCleanupCompleted': {
      recommendedCleanupProgress.value = null;
      showToast(describeRecommendedCleanupOutcome({
        removedCount: Number(data.removed_count ?? 0),
        cleanedCount: Number(data.cleaned_count ?? 0),
        freedBytes: Number(data.freed_bytes ?? 0),
      }), 'success');
      diskUsageVersion.value++;
      break;
    }

    case 'RecommendedCleanupFailed':
      recommendedCleanupProgress.value = null;
      showToast(`Cleanup failed: ${String(data.error ?? 'Unknown error')}`, 'error');
      diskUsageVersion.value++;
      break;

    case 'ModelNotServedObserved':
      // A default moved past the model, or a stored pick now fails. Either
      // way the background rows say something different.
      backgroundModelsVersion.value++;
      break;

    case 'ProxyConfigRejected': {
      // The engine booted with entries in `apis.json` it will not serve, so
      // those proxies 502 when an app or a thread reaches for one.
      //
      // This toast is the fast half, not the guaranteed one. The engine emits
      // it before binding its HTTP port. An ordinary boot therefore reaches no
      // subscriber, and only the notification beside it lands. What this
      // catches is the reconnecting page: a restart the user is watching.
      const rejected = Array.isArray(data.rejected) ? data.rejected : [];
      if (rejected.length === 0) break;
      const detail = rejected
        .map((r) => {
          const entry = r as { provider?: string | null; reason?: string };
          // A null provider is the file itself: unreadable, so no entry to
          // name. Same label the engine logs, so the two read as one refusal.
          return `${entry.provider ?? 'data/config/apis.json'}: ${entry.reason ?? 'unusable'}`;
        })
        .join('; ');
      showToast(`Proxy config problem, ${detail}`, 'error', {
        key: PROXY_CONFIG_REJECTED_TOAST_KEY,
        // The only moment this frame can arrive is just after a restart, which
        // is exactly when `workspaceUnavailable()` suppresses a toast. Without
        // the opt-in the one case it serves is the one case it is dropped in.
        showWhileUnavailable: true,
      });
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
      // dropped events. Reconcile so any "in-flight" UI (Thinking spinner,
      // streaming exchange) agrees with the now-completed backend state.
      const count = (data.count as number) ?? 0;
      console.warn(`[SSE] Stream lagged by ${count} events — reconciling`);
      reconcileAfterEventGap();
      break;
    }

    // ThreadComposeChanged is a *compose report* (ADR 0437). The compose
    // store orders it by revision and lays any pending local edit over it.
    // So our own echo, a delayed frame and a peer's write take one path.
    case 'ThreadComposeChanged': {
      const id = data.id as string;
      // A discarded thread holds no draft, whatever a late frame says.
      const state = threadMap.value.get(id)?.meta.state;
      if (state === undefined || state === 'discarded') break;
      acceptComposeReport(id, parseComposeReport(data as ComposeReportWire));
      break;
    }
  }
}

/** True when this browser discarded the thread itself. The local discard
 *  already updated state synchronously, so its echo must not clobber
 *  keystrokes typed since. */
function discardedHere(event: Extract<ThreadEvent, { type: 'ThreadDiscarded' }>): boolean {
  return event.actor?.kind === 'device' && event.actor.device_id === getDeviceId();
}

function clearComposeIfUnfocused(threadId: string): void {
  if (isComposeFocusedHere(threadId)) return;
  clearDraft(threadId);
}

/** Tool names whose `ToolResult` means `data/` may have changed, so the Files
 *  list must re-read.
 *
 *  This governs the LIST only. The open preview re-reads on the event that
 *  names its path, `Artifact*` or `DataFile*` (`entityReferences.ts`). So a
 *  write to another file never restarts a video the user is watching.
 *  `bash_output` and `BackgroundBashCompleted` name no path anywhere, so they
 *  refresh the list and leave the preview alone. The header Refresh button
 *  covers a log file a background job is still appending to.
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
 *  Refresh button covers it.
 *
 *  Chat links no longer depend on this list being warm. `linkifyPaths` resolves
 *  a full path by shape (ADR 0038), which is what stopped a bash-placed file
 *  from rendering flat. The Files panel and the open preview still do. */
const ARTIFACT_REFRESHING_TOOLS = [
  'write_file', 'edit_file', 'copy_file', 'delete_file', 'import_file', 'bash_output',
];

/** Run the side effects a LIVE frame triggers: opening a form request's form,
 *  refreshes, navigation. A history replay never comes here.
 *
 *  `sourceThreadId` is the thread the event was emitted on. It scopes
 *  `NavigationRequested`, so a navigate from a sibling thread cannot hijack
 *  the page the user is viewing. */
function handleTransientSideEffects(
  event: ThreadEvent | TransientEvent,
  sourceThreadId: string,
): void {
  // A form request: open its form now. One this page misses is found by
  // `syncPendingFormRequests` on the next stream open.
  if (isFormRequest(event)) {
    openFormRequest(sourceThreadId, event);
    return;
  }
  switch (event.type) {
    case 'FormRequestResolved':
      closeResolvedFormRequest(event.request_id, event.outcome, event.actor);
      break;

    case 'PushNotificationRequested':
      if (!isChosenForThread(sourceThreadId)) break;
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

    // A tool call that may have changed `data/` refreshes the Files list.
    // loadArtifacts sets `artifacts` to `failed` via toFailed on error.
    case 'ToolResult': {
      const result = event as { name: string };
      if (ARTIFACT_REFRESHING_TOOLS.includes(result.name)) void loadArtifacts();
      break;
    }

    // The background task finished, so its last writes have landed. Same
    // reasoning as `bash_output` in ARTIFACT_REFRESHING_TOOLS above, including
    // that it names no path, so the open preview is left alone.
    case 'BackgroundBashCompleted':
      void loadArtifacts();
      break;

    // `refresh_app` and the end-of-turn refresh. The tool's reload names the
    // turn's last used device; the end-of-turn one names none, so it reloads
    // the app wherever it is open.
    case 'AppUiRefreshRequested':
      if (isForThisDevice(event.actor)) void refreshAppUI(event.app_id);
      break;

    // A capture names the turn's last used device, and only one of its windows
    // answers: the one showing that app, else any app, else the last-touched
    // window. A second answer would race the real one (engine/tools/app_capture.rs).
    case 'AppUiCaptureRequested':
      // captureAppUI owns its own console.warn telemetry + best-effort
      // postAppCapture surfaces (see apps.ts), so `void` here.
      if (isForThisDevice(event.actor) && isChosenByPreference(
        (window) => window.appId === event.app_id,
        (window) => window.appId !== null,
        () => true,
      )) void captureAppUI(event.app_id, event.request_id, event.save_format);
      break;

    case 'NavigationRequested': {
      let nav;
      try {
        nav = JSON.parse((event as { payload: string }).payload);
      } catch (e) {
        console.error('[SSE] Failed to parse navigation request:', e);
        showToast('Failed to handle navigation request from engine', 'error');
        break;
      }
      routeThreadNavigation(nav, (event as { actor?: DeviceScopeActor }).actor, sourceThreadId);
      break;
    }

    case 'CodingAgentThreadSpawned': {
      const e = event as { cc_thread_id: string; title: string };
      const map = threadMap.value;

      // Only an agent's `run_coding_agent` spawn emits this, on the new thread.
      // Its prompt is the agent's. A pending row on the focused thread is a
      // message the user sent to THAT thread, so it stays there.
      // An agent-sent first message makes the engine store `system`.
      if (!map.has(e.cc_thread_id)) {
        map.set(e.cc_thread_id, makeOptimisticThreadState({
          id: e.cc_thread_id,
          title: e.title,
          channel: 'claude_code',
          initiator: 'system',
          eventsLoaded: false,
        }));
      }
      flushThreadMap();  // Immediate — user needs to see the new thread now
      // The transient-event path in `handleThreadEvent` returns before its
      // bottom bump, so the new thread's own bump fires here.
      bumpThreadEvents(e.cc_thread_id);
      break;
    }
  }
}

export { handleNavigationRequest } from './navigation-request';
