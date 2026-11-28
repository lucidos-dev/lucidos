import { engineVersionStatusVersion } from '../engineVersionStatus';
import { showToast, dismissToast, removeToast, toasts, engineVersionReady, engineVersionPending, engineBuildDeferred, engineUnbuiltHead, engineBuilding, engineBuildDetail, frontendRefreshDetail, enginePendingCommits, engineRestarting, NEW_VERSION_TOAST_KEY, BUILD_FAILED_TOAST_KEY, FRONTEND_UPDATE_DEFERRED_TOAST_KEY, FRONTEND_UPDATE_STRANDED_TOAST_KEY } from '../store';
import { preferencesSettled } from './preferences';
import { engineVersionStatus, rebuildEngine } from '../../api/client';
import { beginSliceLoad, releaseSliceRead, settleSliceRead, sliceLoadStarts, type SliceOf } from '../bootstrapSliceLoads';
import type { EngineVersionStatus, PendingCommits } from '../../api/client';
import { confirmAndRestartEngine } from './chat-changes';
import { noteAnnouncedBuildFailure, noteAnnouncedEngineVersion, wasBuildFailureDismissed, wasEngineVersionDismissed } from '../../hooks/sw-update';
import { engineBuildLogOpen, engineBuildLogVersion } from '../engineBuildLog';
import { errorDetail } from '../../utils/errorDetail';
import { createFailureCounter } from '../../utils/failureCounter';

export function showEngineBuildLog(): void {
  engineBuildLogOpen.value = true;
}

export async function triggerRebuild(mode: 'join' | 'restart' = 'restart'): Promise<void> {
  try {
    await rebuildEngine(mode);
  } catch (e) {
    showToast(`Failed to start rebuild: ${errorDetail(e)}`, 'error');
  }
}

function setEngineBuilding(
  building: boolean,
  status?: EngineVersionStatus,
  commits: PendingCommits | null = null,
): void {
  engineBuilding.value = building;
  if (!building) {
    engineBuildDetail.value = null;
    return;
  }
  engineBuildDetail.value = {
    verifying: status?.build_state === 'verifying',
    elapsedMs: status?.build_elapsed_ms ?? null,
    anchoredAt: Date.now(),
    pendingCommits: commits,
    queuedBehind: status?.build_queued?.holders,
  };
}

function setFrontendRefresh(elapsedMs: number | undefined): void {
  frontendRefreshDetail.value = elapsedMs == null ? null : { elapsedMs, anchoredAt: Date.now() };
}

function groupedCommits(pending: EngineVersionStatus['pending_commits']): PendingCommits | null {
  return pending && Array.isArray(pending.groups) ? pending : null;
}

function setEngineVersionPending(pending: boolean): void {
  engineVersionPending.value = pending;
}

function versionToastIsOpen(): boolean {
  return toasts.value.some((t) => t.key === NEW_VERSION_TOAST_KEY);
}

type VersionAnnouncement = 'ready' | 'pending';

function renderVersionToast(shape: VersionAnnouncement): void {
  const later = {
    label: 'Later',
    onClick: () => { dismissToast(NEW_VERSION_TOAST_KEY); },
  };
  if (shape === 'ready') {
    showToast('New version available.', 'info', {
      key: NEW_VERSION_TOAST_KEY,
      secondaryAction: later,
      action: {
        label: 'Switch',
        onClick: () => { void confirmAndRestartEngine(); },
      },
    });
    return;
  }
  const deferred = engineBuildDeferred.value;
  const noun = deferred?.coding_threads === 1 ? 'thread' : 'threads';
  showToast(deferred ? `New engine version waits for ${deferred.coding_threads} coding ${noun} to finish.` : 'New engine version pending.', 'info', {
    key: NEW_VERSION_TOAST_KEY,
    secondaryAction: later,
    action: {
      label: deferred ? 'Build now' : 'Build new version',
      onClick: () => { void triggerRebuild('join'); },
    },
  });
}

function buildFailureId(status: EngineVersionStatus): string {
  const failure = status.build_failure;
  return JSON.stringify([status.head_commit ?? null, failure?.kind ?? null, failure?.summary ?? null]);
}

function renderBuildFailedToast(status: EngineVersionStatus): void {
  const failureId = buildFailureId(status);
  noteAnnouncedBuildFailure(failureId);
  if (wasBuildFailureDismissed(failureId)) {
    removeToast(BUILD_FAILED_TOAST_KEY);
    return;
  }
  const cause = status.build_failure?.summary ?? 'the engine did not report why';
  showToast(`New engine version failed to build: ${cause}`, 'error', {
    key: BUILD_FAILED_TOAST_KEY,
    secondaryAction: { label: 'Show build log', onClick: showEngineBuildLog },
    action: { label: 'Retry build', onClick: () => { void triggerRebuild(); } },
  });
}

const BUILD_WATCH_STOPPED_TOAST_KEY = 'engine-frontend-build-watch-stopped';
let dismissedBuildWatchStop: number | undefined;

function renderBuildWatchStoppedToast(stopped: EngineVersionStatus['frontend_build_watch_stopped']): void {
  if (!stopped || stopped.since_ms === dismissedBuildWatchStop) {
    removeToast(BUILD_WATCH_STOPPED_TOAST_KEY);
    return;
  }
  const cause = stopped.cause ?? 'cause unknown';
  showToast(`Run ./scripts/web-dev.sh -w <workspace> to start the build-watch again. Exit: ${cause}`, 'warning', {
    key: BUILD_WATCH_STOPPED_TOAST_KEY,
    title: 'Frontend build-watch stopped',
    dismissable: false,
    action: {
      label: 'OK',
      onClick: () => {
        dismissedBuildWatchStop = stopped.since_ms;
        dismissToast(BUILD_WATCH_STOPPED_TOAST_KEY);
      },
    },
  });
}

let lastAnnouncedId: string | undefined;

let reopenedVersionId: string | undefined;

export function resetEngineVersionToastForTest(): void {
  dismissedBuildWatchStop = undefined;
  lastAnnouncedId = undefined;
  reopenedVersionId = undefined;
}

export function openEngineVersionToast(): void {
  if (!engineVersionPending.value) return;
  reopenedVersionId = lastAnnouncedId;
  renderVersionToast('pending');
}

export async function checkEngineVersion(): Promise<void> {
  await readEngineVersion();
}

export function cancelEngineVersionReads(): void {
  ++versionReadGeneration;
}

let versionReadGeneration = 0;
const VERSION_READ_FAILED_TOAST_KEY = 'engine-version-read-failed';
const versionReadFailures = createFailureCounter(3, () => {
  showToast('Could not refresh engine version status. Retrying.', 'error', { key: VERSION_READ_FAILED_TOAST_KEY });
});

function recordVersionReadSuccess(): void {
  versionReadFailures.recordSuccess();
  removeToast(VERSION_READ_FAILED_TOAST_KEY);
}

async function readEngineVersion(): Promise<void> {
  if (engineRestarting.value) return;
  if (!preferencesSettled.value) return;
  const start = beginSliceLoad(VERSION_SLICE);
  if (typeof start !== 'number') return start;
  return new Promise(firstAttemptDone => {
    void retryEngineVersionRead(start, versionReadGeneration, firstAttemptDone);
  });
}

const VERSION_SLICE = 'engine_version_status';

async function retryEngineVersionRead(start: number, generation: number, firstAttemptDone: () => void): Promise<void> {
  const superseded = () => generation !== versionReadGeneration
    || sliceLoadStarts(VERSION_SLICE) !== start || engineRestarting.value;
  let delay = 0;
  try {
    for (;;) {
      if (delay) {
        firstAttemptDone();
        await new Promise(resolve => setTimeout(resolve, delay));
      }
      if (superseded()) break;
      let status: EngineVersionStatus;
      try {
        status = await engineVersionStatus();
      } catch (e) {
        if (superseded()) break;
        console.warn('[engine-update] version-status read failed', e);
        versionReadFailures.recordFailure();
        delay = delay ? Math.min(delay * 4, 30_000) : 250;
        continue;
      }
      if (superseded()) break;
      await settleSliceRead(VERSION_SLICE, start, narrowVersionSlice, undefined, { body: status });
      return;
    }
    releaseSliceRead(VERSION_SLICE, start, narrowVersionSlice, undefined);
  } finally {
    firstAttemptDone();
  }
}

export const engineVersionStatusSlice: SliceOf<'engine_version_status'> = {
  begin: () => {},
  apply: (status) => {
    if (engineRestarting.value || !preferencesSettled.value) return;
    recordVersionReadSuccess();
    applyEngineVersionStatus(status);
  },
  fail: (error) => {
    console.warn('[engine-update] version-status snapshot failed; retrying', error);
    void checkEngineVersion();
  },
};

const narrowVersionSlice: SliceOf<'engine_version_status'> = {
  ...engineVersionStatusSlice,
  fail: (error) => {
    console.error('[engine-update] applying the version status failed', error);
    showToast(`Could not show the engine version status: ${errorDetail(error)}`, 'error');
  },
};

function applyEngineVersionStatus(status: EngineVersionStatus): void {
  renderBuildWatchStoppedToast(status.packaged ? undefined : status.frontend_build_watch_stopped);
  const pendingCommits = groupedCommits(status.pending_commits);
  enginePendingCommits.value = pendingCommits;
  engineBuildDeferred.value = !status.packaged && status.build_deferred
    ? { ...status.build_deferred, anchoredAt: Date.now() } : null;
  engineUnbuiltHead.value = !status.packaged && status.source_behind_head && !status.update_available
    ? status.head_commit ?? null : null;
  if (status.packaged) {
    setEngineBuilding(false);
    setEngineVersionPending(false);
    setFrontendRefresh(undefined);
    return;
  }
  setFrontendRefresh(status.frontend_refresh_elapsed_ms);

  if (status.build_state === 'failed') {
    engineVersionReady.value = false;
    setEngineBuilding(false);
    setEngineVersionPending(false);
    renderBuildFailedToast(status);
    return;
  }
  removeToast(BUILD_FAILED_TOAST_KEY);

  const sharedBuilding = status.shared_build_in_progress === true;

  const ready = status.update_available && status.build_state !== 'building' && status.build_state !== 'verifying';

  const pending =
    status.source_behind_head === true &&
    !status.update_available &&
    status.build_state !== 'building' && status.build_state !== 'verifying' &&
    !sharedBuilding;
  const announcedId = ready ? status.disk_build_id : pending ? status.head_commit : undefined;
  if (announcedId !== undefined) noteAnnouncedEngineVersion(announcedId);
  lastAnnouncedId = announcedId;
  const deferred = announcedId !== undefined && wasEngineVersionDismissed(announcedId);
  const reopened =
    announcedId !== undefined && reopenedVersionId === announcedId && versionToastIsOpen();

  const shape: VersionAnnouncement | null = ready ? 'ready' : pending ? 'pending' : null;
  if (shape !== null && (!deferred || reopened)) {
    renderVersionToast(shape);
  } else {
    removeToast(NEW_VERSION_TOAST_KEY);
  }
  engineVersionReady.value = ready;
  setEngineVersionPending(pending);
  setEngineBuilding(
    status.build_state === 'building' || status.build_state === 'verifying' ||
      (sharedBuilding &&
        status.source_behind_head === true &&
        !status.update_available &&
        status.build_state === 'idle'),
    status,
    pendingCommits,
  );
}

export function handleEngineBuildStateChanged(): void {
  engineBuildLogVersion.value++;
  void checkEngineVersion();
}

export function handleFrontendRefreshStateChanged(): void {
  void checkEngineVersion();
}

export function startEngineUpdateChecks(): void {
  void checkEngineVersion();
}

export function handleEngineVersionStatusChanged(): void {
  engineVersionStatusVersion.value++;
}

export const DEFERRED_HINT_STALE_AFTER_MS = 10_000;

export interface FrontendUpdateDeferredPayload {
  sent_at_ms: number;
}

export interface FrontendUpdateStrandedPayload {
  served_dir: string;
  served_in_worktree: boolean;
  build_error?: string;
  build_watch_stopped?: boolean;
  sent_at_ms: number;
}

export function handleFrontendUpdateDeferred(payload: FrontendUpdateDeferredPayload): void {
  if (Date.now() - payload.sent_at_ms > DEFERRED_HINT_STALE_AFTER_MS) {
    return;
  }
  showToast(
    "Frontend change applied. It'll take effect when you switch to the new version.",
    'info',
    { key: FRONTEND_UPDATE_DEFERRED_TOAST_KEY },
  );
}

export function strandedMessage(payload: FrontendUpdateStrandedPayload): string {
  if (payload.served_in_worktree) {
    return `Frontend change applied but it will not appear: the engine is serving a coding-agent worktree (${payload.served_dir}), which the build-watch never rebuilds. Relaunch the stack from the real checkout.`;
  }
  if (payload.build_watch_stopped) {
    return `Frontend change applied but nothing is rebuilding: the build-watch is not running, so ${payload.served_dir} will stay as it is. Relaunch the stack to restart it.`;
  }
  const failure = payload.build_error?.trim();
  if (failure) {
    return `Frontend change applied but the build is failing, so nothing new is being served: ${failure}`;
  }
  return `Frontend change applied but not served yet. ${payload.served_dir} hasn't rebuilt. It will appear on its own if the build lands; if it doesn't, check the build-watch.`;
}

export function handleFrontendUpdateStranded(payload: FrontendUpdateStrandedPayload): void {
  if (Date.now() - payload.sent_at_ms > DEFERRED_HINT_STALE_AFTER_MS) {
    return;
  }
  const message = strandedMessage(payload);
  showToast(message, 'warning', {
    key: FRONTEND_UPDATE_STRANDED_TOAST_KEY,
    dismissable: false,
    action: {
      label: 'OK',
      onClick: () => { dismissToast(FRONTEND_UPDATE_STRANDED_TOAST_KEY); },
    },
  });
}
