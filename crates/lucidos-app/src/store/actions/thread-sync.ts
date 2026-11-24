import { submitInteractiveRequest } from '../interactiveDeliveryService';

import { flushUndeliveredComposeDrafts } from './compose';
import { isChosenByPreference } from '../windowPeers';
import type { ApplyEstimates, Change, LandingLane, MainCheck } from '../../api/client';

import { threadMap, focusedThreadId, changes, appliedChanges, setAsideChanges, applyingChangeIds, landingLane, applyAllInProgress, applyAllBatch, applyAllCanceling, APPLY_ALL_SUMMARY_TOAST_KEY, applyEstimates, applyPhases, standingApplyThreadIds, generatedTitleIds, codingAgentSessionVersion, setFocusedThread, removingQueuedMessageIds, queuedMessageRemovalKey, UNREAD_HOLD_EVENT_TYPES } from '../store';
import { snapshotsVersion, backgroundModelsVersion, chatModels, findChangeById, memoryRebuildProgress, summaryTreesVersion, backupProgress, backupStatusVersion, backupPreferencesVersion, backupRecipientVersion, intelligenceLadderVersion, recommendedCleanupProgress, diskUsageVersion, promptFootprintVersion, responseStylesVersion, recoveryProgress, showToast, dismissToast, repoSource, TOAST_AUTO_DISMISS_MS } from '../store';
import { describeRecommendedCleanupOutcome } from '../../utils/recommendedCleanup';
import { isFormRequest } from '../thread-events/thread-event-types';
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
import { pairingRequestsVersion } from '../pairingRequests';
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
  handleEngineVersionStatusChanged,
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
import { loadGatewayUsers } from './gatewayUsers';
import { reloadRemoteGatewayLists, remoteGatewayUsers } from './remoteGatewayManage';
import { agentPauseVersion, loadUsers, threadUsersVersion } from './users';
import { loadReaderDevices, readIsMine } from './reader-devices';
import { loadArtifacts, refreshArtifacts } from './artifacts';
import { refreshAppUI, captureAppUI } from './apps';
import { isSelectedFitRenderer, requestWidgetFit } from '../widgetFit';
import { clearWipIfMatches } from './wipPreview';
import { closeResolvedFormRequest, openFormRequest, syncPendingFormRequests } from './form-requests';
import { getDeviceId } from './devices';
import { focusThread } from './threads';
import { refreshRepoView } from './repositories';
import {
  processSSEForReferences,
  refreshLlmConfigured,
  PROVIDER_PREFERENCE_KEYS,
} from './entityReferences';
import { refreshThreadEvents, forgetThreadEventsFailures, markLoadedThreadsStale, refreshArchivedCountAfterFlip } from './thread-loading';
import { refreshThreadList } from './thread-list-refresh';
import { acceptComposeReport, parseComposeReport, type ComposeReportWire } from '../composeSync';
import { clearDraft, setDraft } from '../composeDrafts';
import { removeThreadNavEntries } from './thread-navigation';
import { isComposeFocusedHere } from '../../components/chat/promptFocus';
import { formatBytes } from '../../utils/formatBytes';
import { snapshotCaptureFailureMessage } from './snapshot-capture-failure';

import { routeThreadNavigation } from './navigation-request';
import { dispatchSSEFrame } from '../sseFrame';
import { isForThisDevice, type DeviceScopeActor } from './device-scope';
import { openBackupSettings } from './menu';
import { applyEmbeddingModelStatus } from './backgroundActivity';
import { CHAT_DEFAULT_PREFS, CHAT_LADDER_PREF, LADDER_PREFS, LONG_PROMPT_SHARE_PREF } from '../intelligenceLadderPrefs';
import { applyTreeBackfillFrame, applyTreeBackfillProgress, readyAfterCompleted } from './treeBackfill';
import type { BackfillProgress, EmbeddingModelStatus } from '../../api/types';

const BACKUP_FAILED_TOAST_KEY = 'backup-failed';
const MAIN_PUSH_FAILED_TOAST_KEY = 'main-push-failed';
const MAIN_FOLLOW_REFUSED_TOAST_KEY = 'main-follow-refused';

const PROXY_CONFIG_REJECTED_TOAST_KEY = 'proxy-config-rejected';

let resyncInFlight: Promise<void> | null = null;
let resyncAgain = false;
let repoChangesDebounce: ReturnType<typeof setTimeout> | null = null;

const BACKUP_PREFERENCE_KEYS = new Set([
  'backup_provider',
  'backup_schedule',
  'backup_retention',
]);

const INTELLIGENCE_LADDER_PREFERENCE_KEYS = new Set([
  ...Object.values(LADDER_PREFS).flatMap((p) => [p.ladder, p.model, p.effort]),
  CHAT_LADDER_PREF,
  ...CHAT_DEFAULT_PREFS,
  LONG_PROMPT_SHARE_PREF,
]);

let flushRafId: number | null = null;

function scheduleThreadMapFlush(): void {
  if (flushRafId !== null) return;
  flushRafId = requestAnimationFrame(flushThreadMap);
}

export function flushThreadMap(): void {
  if (flushRafId !== null) {
    cancelAnimationFrame(flushRafId);
    flushRafId = null;
  }
  threadMap.value = new Map(threadMap.value);
}

function findChangeHeadline(threadId: string, changeId: string): string | undefined {
  const naming = findChangeById(changeId)
    ?? changeNamingFromEvents(threadMap.value.get(threadId)?.events.values() ?? [], changeId);
  return naming ? changeHeadline(naming) : undefined;
}

export function handleHostFrame(data: string): void {
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(data);
  } catch (err) {
    console.warn('[SSE] dropping unparseable frame', err);
    return;
  }

  dispatchSSEFrame(parsed, {
    thread: handleThreadEvent,
    system: handleGlobalEvent,
    references: processSSEForReferences,
  });
}

function reconcileAfterEventGap(): void {
  repairGapState();
  void resyncLoadedThreads();
  resyncChangesAndQueue();
  void syncClientUpdateFromBuild();
  readStateMissedBeforeOpen();
}

function readStateMissedBeforeOpen(): void {
  void syncPendingFormRequests();
  refreshArtifacts();
  void loadUnreadNotifications();
  void loadPreferences();
}

export function resyncLoadedThreads(): Promise<void> {
  if (resyncInFlight) {
    resyncAgain = true;
    return resyncInFlight;
  }
  resyncInFlight = (async () => {
    try {
      do {
        resyncAgain = false;
        await resyncPass();
      } while (resyncAgain);
    } finally {
      resyncInFlight = null;
    }
  })();
  return resyncInFlight;
}

async function resyncPass(): Promise<void> {
  markLoadedThreadsStale();
  await refreshThreadList();
  const focused = focusedThreadId.value;
  const refreshFocused = !!focused && !!threadMap.value.get(focused)?.eventsLoaded;
  if (refreshFocused) {
    await refreshThreadEvents(focused, { coalesce: true });
  }
  rereadLoadedDashboards();
}

function resyncChangesAndQueue(): void {
  void refreshChangesState();
  void loadThreadQueue();
}

export function handleThreadEvent(data: Record<string, unknown>, envelopeId?: string): void {
  const threadId = data.thread_id as string;
  const seq = typeof data.seq === 'number' ? data.seq : null;
  const event = data.event as ThreadEvent | TransientEvent;
  const created = typeof data.created === 'string' ? data.created : undefined;
  const eventId = envelopeId ?? (typeof data.event_id === 'string' ? data.event_id : undefined);
  const aggregate = (data.aggregate && typeof data.aggregate === 'object')
    ? (data.aggregate as ThreadAggregate)
    : undefined;
  if (!threadId || !event || typeof event !== 'object' || !('type' in event)) return;
  if (seq !== null && !aggregate) {
    console.warn(`[SSE] persisted event ${event.type} (seq=${seq}) missing aggregate — backend bug`);
  }

  if (event.type === 'ThreadArchived' || event.type === 'ThreadUnarchived') void syncPendingFormRequests();

  const map = threadMap.value;

  let metaChanged = false;

  const rowBornFromFrame = !map.has(threadId);
  if (rowBornFromFrame) {
    if (seq === null) {
      handleTransientSideEffects(event, threadId, eventId);
      return;
    }
    if (!aggregate) return;
    const isThreadStarted = event.type === 'ThreadStarted';
    const startedMode = isThreadStarted ? ((event as Record<string, unknown>).mode as string | undefined) : undefined;
    map.set(threadId, makeOptimisticThreadState({
      id: threadId,
      title: aggregate.title || PENDING_TITLE_PLACEHOLDER,
      channel: aggregate.channel as ThreadMeta['channel'],
      initiator: aggregate.initiator,
      eventsLoaded: isThreadStarted,
      timestamp: aggregate.createdAt,
      ...(isThreadStarted ? {
        state: 'composing' as const,
        status: 'idle' as const,
      } : {}),
    }));
    metaChanged = true;
    if (isThreadStarted) {
      setDraft(threadId, {
        text: '',
        image_hashes: [],
        mode: startedMode === 'claude_code' ? 'claude_code' : 'lucidos',
      });
    }
  }

  const thread = map.get(threadId)!;

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

  if (thread.eventsLoadFailed && seq != null) {
    thread.eventsLoadFailed = false;
    metaChanged = true;
    forgetThreadEventsFailures(threadId);
  }

  const aggregateWasCurrent = aggregate !== undefined && aggregate !== null
    && isSummaryCurrent(thread.meta, aggregate.summaryVersion);
  if (aggregateWasCurrent) liftRowIntent(thread.meta);
  const wasArchived = thread.meta.section === 'archived';
  const handled = handleEvent(map, threadId, seq, event, created, eventId, aggregate);
  if (aggregateWasCurrent && (rowBornFromFrame || wasArchived !== (thread.meta.section === 'archived'))) {
    refreshArchivedCountAfterFlip();
  }
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
  if (UNREAD_HOLD_EVENT_TYPES.has(event.type)) metaChanged = true;

  if (aggregateWasCurrent && overlayRowIntent(thread.meta)) metaChanged = true;

  if (event.type === 'MessageReceived' || event.type === 'UserOnlyMessagePosted') {
    if (thread.meta.state !== 'active') { thread.meta.state = 'active'; metaChanged = true; }
  }
  if (event.type === 'AgentPaused' || event.type === 'AgentResumed') agentPauseVersion.value++;
  if (event.type === 'RootThreadDriverTaken' || event.type === 'RootThreadDriverReleased'
      || event.type === 'RootThreadWatched' || event.type === 'RootThreadUnwatched') {
    threadUsersVersion.value++;
  }
  if (event.type === 'ThreadDiscarded') {
    if (thread.meta.state !== 'discarded') { thread.meta.state = 'discarded'; metaChanged = true; }
    if (!discardedHere(event)) {
      if (!isComposeFocusedHere(threadId)) {
        if (focusedThreadId.value === threadId) setFocusedThread(null);
        removeThreadNavEntries(threadId);
      }
      clearComposeIfUnfocused(threadId);
    }
  }

  if (event.type === 'SessionStarted' || event.type === 'ContinuationStarted'
      || event.type === 'SessionEnded' || event.type === 'CodingAgentUserMessageSent'
      || event.type === 'CodingAgentIdled' || event.type === 'CodingAgentSettingsChanged') {
    codingAgentSessionVersion.value++;
  }


  handleTransientSideEffects(event, threadId, eventId);

  if (event.type === 'ChangeApplied') {
    const lastPhase = clearApplyPhase(threadId);
    const inBatch = isBatchMember(applyAllBatch.value, event.change_id);
    resolveBatchMember(event.change_id);
    const desc = event.change_id ? findChangeHeadline(threadId, event.change_id) : undefined;
    const requiresRestart = !!event.requires_restart;
    const applyKey = `applying-${threadId}`;
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
    clearApplyPhase(threadId);
    resolveBatchMember(event.change_id);
  }

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

  if (event.type === 'ChangeApplied' || event.type === 'ChangeDiscarded' || event.type === 'ChangeReverted') {
    if (threadId === focusedThreadId.value) {
      document.dispatchEvent(new Event('reveal-mobile-bars'));
    }
    clearWipIfMatches((wipTid) => wipTid === threadId);
  }
  if (event.type === 'ThreadArchived') {
    clearWipIfMatches((wipTid) => wipTid === threadId);
  }

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

  if (event.type === 'MergeConflictDetected' && event.change_id
      && !applyingChangeIds.value.has(event.change_id)) {
    applyingChangeIds.value = new Set([...applyingChangeIds.value, event.change_id]);
  }

  if (event.type === 'MergeConflictDetected' || event.type === 'MissingHardeningDetected') {
    const phase = event.type === 'MergeConflictDetected' ? 'resolving-conflict' : 'hardening';
    setApplyPhase(threadId, { phase, eventId: eventId ?? null, startedAt: created ?? null });
  } else if ((event.type === 'ChangeHardened' || event.type === 'ChangeProposed')
      && applyPhases.value.has(threadId)) {
    setApplyPhase(threadId, { phase: 'merging', eventId: null, startedAt: null });
  }

  bumpThreadEvents(threadId);

  if (metaChanged) {
    scheduleThreadMapFlush();
  }
}

function resolveBatchMember(changeId: string | undefined): void {
  const batch = applyAllBatch.value;
  if (!batch || !changeId || !isBatchMember(batch, changeId)
      || batch.resolvedChangeIds.includes(changeId)) return;
  applyAllBatch.value = { ...batch, resolvedChangeIds: [...batch.resolvedChangeIds, changeId] };
}

function forgetStandingApply(threadId: string | undefined): void {
  if (!threadId || !standingApplyThreadIds.value.has(threadId)) return;
  const next = new Set(standingApplyThreadIds.value);
  next.delete(threadId);
  standingApplyThreadIds.value = next;
}

export function handleGlobalEvent(type: string, data: Record<string, unknown>): void {
  switch (type) {
    case 'NotificationCreated':
      if (typeof data.id === 'string') noteArrivalWhilePresent(data.id);
      handleNotificationSSE();
      break;

    case 'NotificationRead':
      if (typeof data.id === 'string' && readIsMine(data.actor)) dropNotificationToast(data.id);
      handleNotificationSSE();
      break;

    case 'NotificationsAllRead':
      if (readIsMine(data.actor)) dropAllNotificationToasts();
      handleNotificationSSE();
      break;

    case 'ThreadsDeleted': {
      const ids = Array.isArray(data.thread_ids) ? (data.thread_ids as string[]) : [];
      dropDeletedThreads(ids);
      void refreshThreadList();
      handleNotificationSSE();
      void loadThreadQueue();
      summaryTreesVersion.value++;
      break;
    }

    case 'PresenceCheck': {
      const payload = data as unknown as PresenceCheckPayload;
      const outcome = handlePresenceCheck(payload);
      if (outcome === null || outcome === 'not_shown') noteArrivalWhileAway(payload.notification_id);
      break;
    }

    case 'NativePushRequested':
      handleNativePushRequested(data as unknown as NativePushRequestedPayload);
      break;

    case 'NativePushDismissRequested':
      handleNativePushDismiss(data as unknown as NativePushDismissRequestedPayload);
      break;

    case 'PreferencesChanged':
      promptFootprintVersion.value++;
      void loadPreferences().then(() => {
        if (data.key === 'theme') void refreshActiveTheme();
        return syncClientUpdateFromBuild();
      }).catch(() => {  });
      if (BACKUP_PREFERENCE_KEYS.has(String(data.key ?? ''))) {
        backupPreferencesVersion.value++;
      }
      if (INTELLIGENCE_LADDER_PREFERENCE_KEYS.has(String(data.key ?? ''))) {
        intelligenceLadderVersion.value++;
      }
      if (data.key === CHAT_LADDER_PREF && chatModels.value.status === 'loaded') {
        void loadChatModels();
      }
      if (PROVIDER_PREFERENCE_KEYS.has(String(data.key ?? ''))) {
        refreshLlmConfigured();
      }
      if (String(data.key ?? '') === 'response_styles') {
        responseStylesVersion.value++;
      }
      void loadReleaseNotices();
      break;
    case 'LanguageSet':
    case 'TimezoneSet':
      void loadPreferences();
      break;

    case 'UsersChanged':
      void loadReaderDevices();
      void loadGatewayUsers();
      void loadUsers();
      if (remoteGatewayUsers.value.status !== 'not-loaded') void reloadRemoteGatewayLists();
      break;

    case 'PairingRequestsChanged':
      pairingRequestsVersion.value++;
      break;

    case 'ReleaseNoticeResolved':
      void loadReleaseNotices();
      break;

    case 'AppUiRefreshRequested':
      void refreshAppUI(data.app_id as string | undefined);
      break;

    case 'FrontendUpdateDeferred':
      handleFrontendUpdateDeferred(data as unknown as FrontendUpdateDeferredPayload);
      break;

    case 'FrontendUpdateStranded':
      handleFrontendUpdateStranded(data as unknown as FrontendUpdateStrandedPayload);
      break;

    case 'ServedFrontendAdvanced':
      void syncClientUpdateFromBuild();
      break;

    case 'FrontendPreviewStarted':
      handleFrontendPreviewStarted(data as { thread_id?: string; port?: number });
      break;

    case 'FrontendPreviewStopped':
      handleFrontendPreviewStopped(data as { thread_id?: string });
      break;

    case 'EngineVersionStatusChanged':
      handleEngineVersionStatusChanged();
      break;

    case 'EngineBuildStateChanged':
      handleEngineBuildStateChanged();
      break;

    case 'FrontendRefreshStateChanged':
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
      applyTreeBackfillFrame({ state: 'off', started: true });
      summaryTreesVersion.value++;
      break;

    case 'EmbeddingModelStatusChanged': {
      applyEmbeddingModelStatus({
        model_id: String(data.model_id ?? ''),
        load_state: data.load_state as EmbeddingModelStatus['load_state'],
      });
      break;
    }

    case 'ApplyAllBatchStarted': {
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
      handleLaneFrame(type, data);
      break;

    case 'MainPushFailed': {
      const reason = typeof data.reason === 'string' && data.reason ? data.reason : 'Unknown error';
      showToast(`Could not push main to origin: ${reason}`, 'error', { key: MAIN_PUSH_FAILED_TOAST_KEY });
      break;
    }

    case 'MainFollowRefused': {
      const reason = typeof data.reason === 'string' && data.reason ? data.reason : 'Unknown error';
      showToast(`Could not follow main from origin: ${reason}`, 'error', { key: MAIN_FOLLOW_REFUSED_TOAST_KEY });
      break;
    }

    case 'MainCheckStarted':
    case 'MainCheckRunLinked':
    case 'MainCheckPassed':
    case 'MainCheckFailed':
    case 'MainCheckAbandoned':
    case 'MainCheckSuperseded':
    case 'MainCheckRepairStarted':
    case 'MainCheckRedClosed':
      handleMainCheckFrame(type, data);
      break;

    case 'StandingApplyArmed': {
      const threadId = data.thread_id as string | undefined;
      if (threadId) {
        standingApplyThreadIds.value = new Set([...standingApplyThreadIds.value, threadId]);
      }
      break;
    }

    case 'StandingApplyFired': {
      forgetStandingApply(data.thread_id as string | undefined);
      break;
    }

    case 'StandingApplyDropped': {
      const threadId = data.thread_id as string | undefined;
      forgetStandingApply(threadId);
      const reason = typeof data.reason === 'string' ? data.reason : '';
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
      if (repoChangesDebounce) clearTimeout(repoChangesDebounce);
      repoChangesDebounce = setTimeout(() => {
        const currentRepo = repoSource.value;
        if (currentRepo) void refreshRepoView(currentRepo);
      }, 300);
      break;
    }

    case 'SnapshotCreated':
    case 'SnapshotDeleted':
    case 'SnapshotKeepChanged':
      snapshotsVersion.value++;
      break;
    case 'SnapshotCaptureFailed':
      snapshotsVersion.value++;
      showToast(snapshotCaptureFailureMessage(data.reason), 'error', { title: 'Workspace snapshots' });
      break;

    case 'BackupProgress': {
      const phase = (data.phase as string) ?? '';
      const progress = (data.progress as number) ?? 0;
      const total = (data.total as number) ?? 0;
      backupProgress.value = { phase, progress, total };
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
      backgroundModelsVersion.value++;
      break;

    case 'ProxyConfigRejected': {
      const rejected = Array.isArray(data.rejected) ? data.rejected : [];
      if (rejected.length === 0) break;
      const detail = rejected
        .map((r) => {
          const entry = r as { provider?: string | null; reason?: string };
          return `${entry.provider ?? 'data/config/apis.json'}: ${entry.reason ?? 'unusable'}`;
        })
        .join('; ');
      showToast(`Proxy config problem, ${detail}`, 'error', {
        key: PROXY_CONFIG_REJECTED_TOAST_KEY,
        showWhileUnavailable: true,
      });
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
      const count = (data.count as number) ?? 0;
      console.warn(`[SSE] Stream lagged by ${count} events — reconciling`);
      reconcileAfterEventGap();
      break;
    }

    case 'OrdinarySubmissionAccepted':
    case 'DraftSubmissionAccepted': {
      void refreshThreadList();
      break;
    }

    case 'ThreadComposeChanged': {
      const id = data.id as string;
      const state = threadMap.value.get(id)?.meta.state;
      if (state === undefined || state === 'discarded') break;
      acceptComposeReport(id, parseComposeReport(data as ComposeReportWire));
      break;
    }
  }
}

function discardedHere(event: Extract<ThreadEvent, { type: 'ThreadDiscarded' }>): boolean {
  return event.actor?.kind === 'device' && event.actor.device_id === getDeviceId();
}

function clearComposeIfUnfocused(threadId: string): void {
  if (isComposeFocusedHere(threadId)) return;
  clearDraft(threadId);
}

const ARTIFACT_REFRESHING_TOOLS = [
  'write_file', 'edit_file', 'copy_file', 'delete_file', 'import_file', 'bash_output',
];

function handleTransientSideEffects(
  event: ThreadEvent | TransientEvent,
  sourceThreadId: string,
  eventId?: string,
): void {
  if (isFormRequest(event)) {
    openFormRequest(sourceThreadId, event);
    return;
  }
  switch (event.type) {
    case 'FormRequestResolved':
      closeResolvedFormRequest(event.request_id, event.outcome, event.actor);
      break;

    case 'PushNotificationRequested':
      if (!isForThisDevice(event.actor)) break;
      submitInteractiveRequest({ kind: 'push-enable', payload: {}, preference: { threadId: sourceThreadId } }, eventId);
      break;


    case 'ToolResult': {
      const result = event as { name: string };
      if (ARTIFACT_REFRESHING_TOOLS.includes(result.name)) void loadArtifacts();
      break;
    }

    case 'BackgroundBashCompleted':
      void loadArtifacts();
      break;

    case 'AppUiRefreshRequested':
      if (isForThisDevice(event.actor)) void refreshAppUI(event.app_id);
      break;

    case 'AppUiCaptureRequested':
      if (isForThisDevice(event.actor) && isChosenByPreference(
        (window) => window.appId === event.app_id,
        (window) => window.appId !== null,
        () => true,
      )) void captureAppUI(event.app_id, event.request_id, event.save_format);
      break;

    case 'WidgetFitRequested':
      if (event.renderer_session_id
        ? isSelectedFitRenderer(event.renderer_session_id, event.renderer_device_id, getDeviceId())
        : isForThisDevice(event.actor) && isChosenByPreference(
          (window) => window.focusedThreadId === sourceThreadId, () => true,
        )) {
        requestWidgetFit({
          requestId: event.request_id,
          appId: event.app_id,
          sizes: event.sizes,
          scale: event.ui_scale,
          scaleDevice: event.scale_device_id,
        });
      }
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
      routeThreadNavigation(nav, (event as { actor?: DeviceScopeActor }).actor, sourceThreadId, eventId);
      break;
    }

    case 'CodingAgentThreadSpawned': {
      const e = event as { cc_thread_id: string; title: string };
      const map = threadMap.value;

      if (!map.has(e.cc_thread_id)) {
        map.set(e.cc_thread_id, makeOptimisticThreadState({
          id: e.cc_thread_id,
          title: e.title,
          channel: 'claude_code',
          initiator: 'system',
          eventsLoaded: false,
        }));
      }
      flushThreadMap();
      bumpThreadEvents(e.cc_thread_id);
      break;
    }
  }
}

export { handleNavigationRequest } from './navigation-request';

export function onStreamOpen(reopenedAfterError: boolean): void {
  if (reopenedAfterError) reconcileAfterEventGap();
  else {
    handleEngineVersionStatusChanged();
    readStateMissedBeforeOpen();
  }
}

export function flushAdmittedComposeDrafts(): void {
  flushUndeliveredComposeDrafts();
}

export { connectThreadEvents, disconnectThreadEvents, reviveThreadEvents } from './thread-stream';
export { rebuildCorruptedThreadEvents } from './thread-event-repair';
