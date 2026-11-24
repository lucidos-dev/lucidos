import { showToast, showConfirm, dismissToast, removeToast, changes, appliedChanges, setAsideChanges, lazyChanges, findChangeById, changesHasMore, changesLoadingMore, restartRequired, restartGroups, applyingChangeIds, applyAllInProgress, applyAllBatch, applyAllCanceling, applyEstimates, standingApplyThreadIds, armingStandingApplyThreadIds, engineRestarting, engineRestartNewVersion, engineStartedAt, engineVersion, latestEngineVersion, engineNewVersionReady, enginePendingCommits, NEW_VERSION_TOAST_KEY, FRONTEND_UPDATE_DEFERRED_TOAST_KEY } from '../store';
import { changeToastMessage } from './changeToast';
import { engineVersionStatusVersion } from '../engineVersionStatus';
import { restartConfirmCopy } from '../restartConfirmCopy';
import { loadedOr, toFailed } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import type { Loadable } from '../types';
import type { RestartGroup } from '../store';
import { applyChange as apiApply, discardChange as apiDiscard, setAsideChange as apiSetAside, bringBackChange as apiBringBack, applyAllChanges as apiApplyAll, cancelApplyAllChanges as apiCancelApplyAll, discardAllChanges as apiDiscardAll, revertChange as apiRevert, fetchChanges as apiFetchChanges, getChangeById as apiGetChangeById, armStandingApply as apiArmStandingApply, disarmStandingApply as apiDisarmStandingApply, restartEngine, ApiError, isTransportError } from '../../api/client';
import { isTauri } from '../../utils/platform';
import { restartRestartsGateway } from '../enginePackaging';
import { invoke } from '../../utils/tauri';
import { isNewerBuildId } from '../../utils/version';
import { errorDetail, isAbortError } from '../../utils/errorDetail';
import { reconcileApplyProgress } from './applyProgress';
import { setLandingLaneFromServer } from './landingLane';
import { setMainCheckFromServer } from './mainCheck';
import type { Change } from '../../api/client';

export const RESTART_FAILURE_TOAST_KEY = 'restart-required';
export const RESTART_LS_KEY = 'lucidos-restart-required';

export const APPLY_NEW_VERSION_TOOLTIP =
  'Applies now (non-disruptive). A new engine version builds in the background; you’ll be prompted to switch to it when ready.';

export const RESTART_GROUPS_LS_KEY = 'lucidos-restart-groups';
const LEGACY_RESTART_REASONS_LS_KEY = 'lucidos-restart-reasons';

export const RESTART_IN_FLIGHT_LS_KEY = 'lucidos-restart-in-flight';

interface RestartInFlight {
  startedAt: string | null;
  newVersion: boolean;
}

function markRestartInFlight(newVersion: boolean): void {
  const payload: RestartInFlight = {
    startedAt: engineStartedAt.value,
    newVersion,
  };
  localStorage.setItem(RESTART_IN_FLIGHT_LS_KEY, JSON.stringify(payload));
}

export function clearRestartInFlight(): void {
  localStorage.removeItem(RESTART_IN_FLIGHT_LS_KEY);
}

function readRestartInFlight(): RestartInFlight | null {
  const raw = localStorage.getItem(RESTART_IN_FLIGHT_LS_KEY);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as Partial<RestartInFlight>;
    return {
      startedAt: typeof parsed.startedAt === 'string' ? parsed.startedAt : null,
      newVersion: parsed.newVersion === true,
    };
  } catch {
    localStorage.removeItem(RESTART_IN_FLIGHT_LS_KEY);
    return null;
  }
}

export function addRestartGroup(group: RestartGroup): void {
  const existing = restartGroups.value;
  const idx = existing.findIndex(g => g.threadId === group.threadId);
  if (idx === -1) {
    restartGroups.value = [...existing, group];
  } else {
    const merged: RestartGroup = {
      threadId: group.threadId,
      threadTitle: group.threadTitle,
      commits: dedupePreservingOrder([...existing[idx].commits, ...group.commits]),
    };
    const next = existing.slice();
    next[idx] = merged;
    restartGroups.value = next;
  }
  persistRestartGroups();
  restartRequired.value = true;
  syncRestartState();
}

function dedupePreservingOrder(items: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const item of items) {
    if (!seen.has(item)) {
      seen.add(item);
      out.push(item);
    }
  }
  return out;
}

function persistRestartGroups(): void {
  const groups = restartGroups.value;
  if (groups.length > 0) {
    localStorage.setItem(RESTART_GROUPS_LS_KEY, JSON.stringify(groups));
    localStorage.removeItem(LEGACY_RESTART_REASONS_LS_KEY);
  } else {
    localStorage.removeItem(RESTART_GROUPS_LS_KEY);
  }
}

export async function initiateEngineRestart(): Promise<void> {
  removeToast(NEW_VERSION_TOAST_KEY);
  removeToast(FRONTEND_UPDATE_DEFERRED_TOAST_KEY);
  const newVersion = engineNewVersionReady();
  engineRestartNewVersion.value = newVersion;
  engineRestarting.value = true;
  markRestartInFlight(newVersion);
  try {
    if (restartRestartsGateway()) {
      await invoke('restart_service');
      return;
    }
    await restartEngine();
  } catch (e) {
    if (e instanceof ApiError) {
      engineRestarting.value = false;
      clearRestartInFlight();
      showToast(`Restart failed: ${e.reason}`, 'error', { key: RESTART_FAILURE_TOAST_KEY });
      engineVersionStatusVersion.value++;
      return;
    }
    if (typeof e === 'string') {
      engineRestarting.value = false;
      clearRestartInFlight();
      showToast(`Restart failed: ${e}`, 'error', { key: RESTART_FAILURE_TOAST_KEY });
      engineVersionStatusVersion.value++;
      return;
    }
  }
}

export async function confirmAndRestartEngine(): Promise<void> {
  const extraAction = isTauri()
    ? {
        label: 'Restart App',
        onClick: () => {
          invoke('restart_app').catch((e: unknown) => {
            showToast(`Failed to restart app: ${e}`, 'error');
          });
        },
      }
    : undefined;
  const copy = restartConfirmCopy(
    engineNewVersionReady(),
    enginePendingCommits.value,
    restartGroups.value,
  );
  const confirmed = await showConfirm(copy.message, copy.okLabel, {
    title: copy.title,
    cancelLabel: copy.cancelLabel,
    extraAction,
    variant: 'default',
    details: copy.details,
  });
  if (confirmed) {
    await initiateEngineRestart();
  }
}

function isEngineOutdated(): boolean {
  const ev = engineVersion.value;
  const lev = latestEngineVersion.value;
  return !!(ev && lev && isNewerBuildId(lev, ev));
}

export function syncRestartState(): void {
  if (engineRestarting.value) {
    if (restartRequired.value) localStorage.setItem(RESTART_LS_KEY, 'true');
    return;
  }
  if (restartRequired.value) {
    localStorage.setItem(RESTART_LS_KEY, 'true');
  } else {
    localStorage.removeItem(RESTART_LS_KEY);
    restartGroups.value = [];
    persistRestartGroups();
    dismissToast(RESTART_FAILURE_TOAST_KEY);
  }
}

function restoreRestartGroupsFromStorage(): void {
  try {
    const saved = localStorage.getItem(RESTART_GROUPS_LS_KEY);
    if (saved) restartGroups.value = JSON.parse(saved) as RestartGroup[];
  } catch {
    localStorage.removeItem(RESTART_GROUPS_LS_KEY);
  }
}

export function restoreRestartState(): void {
  localStorage.removeItem(LEGACY_RESTART_REASONS_LS_KEY);

  const inFlight = readRestartInFlight();
  if (inFlight) {
    restoreRestartGroupsFromStorage();
    restartRequired.value = true;
    engineRestartNewVersion.value = inFlight.newVersion;
    engineRestarting.value = true;
    engineStartedAt.value = inFlight.startedAt;
    return;
  }

  if (localStorage.getItem(RESTART_LS_KEY) === 'true') {
    restoreRestartGroupsFromStorage();
    restartRequired.value = true;
    syncRestartState();
  }
}

export function refreshChangesState(): Promise<void> {
  return loadSlice('changes_state', changesStateSlice, () => apiFetchChanges({ limit: 15 })
    .catch(e => {
      if ((e instanceof DOMException && e.name === 'TimeoutError') || isTransportError(e)) {
        return apiFetchChanges({ limit: 15 });
      }
      throw e;
    }));
}

export const changesStateSlice: SliceOf<'changes_state'> = {
  begin: () => {},
  apply:
    (state) => {
      const applied = state.applied || [];
      const previousPending = loadedOr(changes.value, []);
      changes.value = { status: 'loaded', data: state.pending };
      appliedChanges.value = { status: 'loaded', data: applied };
      setAsideChanges.value = { status: 'loaded', data: state.set_aside ?? [] };
      changesHasMore.value = state.has_more_applied;
      restartRequired.value = state.restart_required || isEngineOutdated();
      restartGroups.value = (state.restart_groups ?? []).map(g => ({
        threadId: g.thread_id ?? '',
        threadTitle: g.thread_title ?? 'unknown',
        commits: g.commits,
      }));
      const batch = state.apply_all_batch;
      applyAllBatch.value = batch
        ? {
          changeIds: batch.change_ids,
          resolvedChangeIds: batch.resolved_change_ids,
          applyingChangeIds: batch.applying_change_ids ?? [],
          resolvingChangeIds: batch.resolving_change_ids ?? [],
        }
        : null;
      if (!state.apply_all_in_progress) applyAllCanceling.value = false;
      applyAllInProgress.value = state.apply_all_in_progress ?? false;
      setLandingLaneFromServer(state.landing_lane);
      setMainCheckFromServer(state.main_check);
      standingApplyThreadIds.value = new Set(state.standing_apply_thread_ids ?? []);
      if (state.apply_estimates) applyEstimates.value = state.apply_estimates;
      reconcileApplyProgress(previousPending, state.pending, applied);
      syncRestartState();
    },
  fail:
    (e) => {
      if (isAbortError(e) || isTransportError(e)) return;
      changes.value = toFailed<Change[]>(e);
      appliedChanges.value = toFailed<Change[]>(e);
      setAsideChanges.value = toFailed<Change[]>(e);
      showToast(`Failed to fetch changes: ${errorDetail(e)}`, 'error');
    },
};

export async function applySingleChange(id: string): Promise<void> {
  try {
    const result = await apiApply(id);
    if (result.status === 'applied' && result.version_build === 'deferred') {
      const count = result.deferred_coding_threads;
      showToast(count == null ? result.message : `Applied. New version waits for ${count} coding ${count === 1 ? 'thread' : 'threads'}.`, 'info');
    }
    if (result.status === 'applied' || result.status === 'noop') {
      const pending = changes.value;
      if (pending.status === 'loaded') {
        changes.value = { status: 'loaded', data: pending.data.filter((c) => c.id !== id) };
      }
    } else if (result.status === 'hardening') {
      applyingChangeIds.value = new Set([...applyingChangeIds.value, id]);
    }
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to apply change', 'error');
  }
}

const DISCARD_ONE_CONFIRM = 'Discard this change? This cannot be undone.';
const DISCARD_ALL_CONFIRM =
  'Discard every change whose thread has finished? Threads still working keep theirs. This cannot be undone.';

export async function discardSingleChange(id: string): Promise<void> {
  if (!(await showConfirm(DISCARD_ONE_CONFIRM, 'Discard', { variant: 'danger' }))) return;
  try {
    await apiDiscard(id);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to discard change', 'error');
  }
}

export async function setAsideSingleChange(id: string): Promise<void> {
  try {
    await apiSetAside(id);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to set the change aside', 'error');
  }
}

export async function bringBackSingleChange(id: string): Promise<void> {
  try {
    await apiBringBack(id);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to bring the change back', 'error');
  }
}

export async function applyAllChanges(): Promise<void> {
  if (applyAllInProgress.value) return;
  applyAllInProgress.value = true;
  try {
    await apiApplyAll();
  } catch (e) {
    applyAllInProgress.value = false;
    showToast(errorDetail(e) || 'Failed to apply changes', 'error');
  }
}

export async function cancelApplyAllBatch(): Promise<void> {
  applyAllCanceling.value = true;
  try {
    await apiCancelApplyAll();
  } catch (e) {
    applyAllCanceling.value = false;
    showToast(errorDetail(e) || 'Failed to cancel apply', 'error');
  }
}

export async function discardAllChanges(): Promise<void> {
  if (!(await showConfirm(DISCARD_ALL_CONFIRM, 'Discard', { variant: 'danger' }))) return;
  try {
    const result = await apiDiscardAll();
    if (result.failed > 0) {
      showToast(`${result.failed} change(s) failed to discard: ${result.errors.join('; ')}`, 'error');
    }
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to discard changes', 'error');
  }
}

export const STANDING_APPLY_CANCELED = 'Canceled.';

function setStandingApplyArmed(threadId: string, armed: boolean): void {
  if (standingApplyThreadIds.value.has(threadId) === armed) return;
  const next = new Set(standingApplyThreadIds.value);
  if (armed) next.add(threadId);
  else next.delete(threadId);
  standingApplyThreadIds.value = next;
}

export async function armStandingApply(threadId: string, changeId?: string): Promise<void> {
  if (armingStandingApplyThreadIds.value.has(threadId)) return;
  armingStandingApplyThreadIds.value = new Set([...armingStandingApplyThreadIds.value, threadId]);
  const wasArmed = standingApplyThreadIds.value.has(threadId);
  setStandingApplyArmed(threadId, true);
  try {
    await apiArmStandingApply(threadId, changeId);
  } catch (e) {
    setStandingApplyArmed(threadId, wasArmed);
    showToast(
      changeToastMessage('Failed to arm the standing apply', threadId, errorDetail(e)),
      'error',
    );
  } finally {
    const next = new Set(armingStandingApplyThreadIds.value);
    next.delete(threadId);
    armingStandingApplyThreadIds.value = next;
  }
}

export async function disarmStandingApply(threadId: string): Promise<void> {
  if (armingStandingApplyThreadIds.value.has(threadId)) return;
  armingStandingApplyThreadIds.value = new Set([...armingStandingApplyThreadIds.value, threadId]);
  const wasArmed = standingApplyThreadIds.value.has(threadId);
  setStandingApplyArmed(threadId, false);
  try {
    await apiDisarmStandingApply(threadId);
  } catch (e) {
    if (e instanceof ApiError && e.httpCode === 404) return;
    setStandingApplyArmed(threadId, wasArmed);
    showToast(
      changeToastMessage('Failed to cancel the standing apply', threadId, errorDetail(e)),
      'error',
    );
  } finally {
    const next = new Set(armingStandingApplyThreadIds.value);
    next.delete(threadId);
    armingStandingApplyThreadIds.value = next;
  }
}

export async function armStandingApplies(pending: Change[]): Promise<void> {
  const request = Promise.all(
    pending.flatMap((c) => (c.thread_id ? [armStandingApply(c.thread_id, c.id)] : [])),
  );
  sectionArmRequest = request;
  try {
    await request;
  } finally {
    if (sectionArmRequest === request) sectionArmRequest = null;
  }
}

let sectionArmRequest: Promise<unknown> | null = null;

export async function disarmStandingApplies(threadIds: string[]): Promise<void> {
  await sectionArmRequest?.catch(() => undefined);
  await Promise.all(threadIds.map((id) => disarmStandingApply(id)));
}

export async function revertChange(id: string): Promise<void> {
  try {
    await apiRevert(id);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to revert change', 'error');
  }
}

export function applyChangeSummarized(id: string, summary: string, description: string): void {
  const cached = lazyChanges.value.get(id);
  if (cached?.status !== 'loaded' || cached.data.description !== description) return;
  setLazyChange(id, { status: 'loaded', data: { ...cached.data, summary } });
}

function setLazyChange(id: string, value: Loadable<Change>): void {
  const next = new Map(lazyChanges.value);
  next.set(id, value);
  lazyChanges.value = next;
}

export async function ensureChangeLoaded(id: string): Promise<void> {
  if (findChangeById(id)) return;
  if (lazyChanges.value.has(id)) return;

  setLazyChange(id, { status: 'loading' });
  try {
    const change = await apiGetChangeById(id);
    setLazyChange(id, { status: 'loaded', data: change });
  } catch (e) {
    setLazyChange(id, toFailed<Change>(e));
  }
}

const MAX_CHANGES_PAGE_RETRIES = 3;

export async function loadMoreChanges(): Promise<void> {
  if (changesLoadingMore.value || !changesHasMore.value) return;

  changesLoadingMore.value = true;
  try {
    for (let attempt = 0; attempt < MAX_CHANGES_PAGE_RETRIES; attempt++) {
      const base = appliedChanges.value;
      if (base.status !== 'loaded' || base.data.length === 0) return;
      const resolvedAt = base.data[base.data.length - 1].resolved_at;
      if (!resolvedAt) return;
      const beforeTs = new Date(resolvedAt).getTime() / 1000;

      const data = await apiFetchChanges({ limit: 15, before: beforeTs });

      const after = appliedChanges.value;
      if (after.status !== 'loaded') return;
      if (after !== base) continue;
      const seen = new Set(after.data.map((c) => c.id));
      const fresh = (data.applied || []).filter((c) => !seen.has(c.id));
      appliedChanges.value = { status: 'loaded', data: [...after.data, ...fresh] };
      changesHasMore.value = data.has_more_applied;
      return;
    }
  } catch (e) {
    showToast(`Failed to load more changes: ${errorDetail(e)}`, 'error');
  } finally {
    changesLoadingMore.value = false;
  }
}
