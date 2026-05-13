import { showToast, dismissToast, changes, appliedChanges, lazyChanges, findChangeById, changesHasMore, changesLoadingMore, restartRequired, updateAvailable, restartGroups, applyingChangeIds, toasts, engineRestarting, engineVersion, latestEngineVersion, threadMap } from '../store';
import { toFailed } from '../types';
import type { Loadable } from '../types';
import type { RestartGroup } from '../store';
import { applyChange as apiApply, discardChange as apiDiscard, applyAllChanges as apiApplyAll, discardAllChanges as apiDiscardAll, revertChange as apiRevert, fetchChanges as apiFetchChanges, getChangeById as apiGetChangeById, restartEngine, ApiError } from '../../api/client';
import { isNewerVersion } from '../../utils/version';
import { errorDetail } from '../../utils/errorDetail';
import { PENDING_TITLE_PLACEHOLDER } from '../thread-events';
import { focusThread } from './threads';
import type { Change } from '../../api/client';
import type { ToastAction } from '../types';

export const RESTART_TOAST_KEY = 'restart-required';
export const RESTART_LS_KEY = 'lucidos-restart-required';
/** Fingerprint of the restart-needing change set the user has explicitly
 *  dismissed. While this matches the current fingerprint, the toast stays
 *  hidden — a new commit or new thread group changes the fingerprint and
 *  the toast comes back. */
export const RESTART_DISMISSED_FP_LS_KEY = 'lucidos-restart-dismissed-fp';

/** Timestamp (ms) when this page's JavaScript was loaded.
 *  Changes applied before this time are already reflected in the running client. */
const PAGE_LOADED_AT = Date.now();

const CLIENT_FILE_RE = /\.(ts|tsx|css|html|js|jsx)$/;

/** Restart-required changes reload on reconnect; offering Refresh too would race. */
export function appliedToastRefreshAction(
  requiresRestart: boolean,
  clientUpdate: boolean,
): ToastAction | undefined {
  if (!clientUpdate || requiresRestart) return undefined;
  return { label: 'Refresh', onClick: () => window.location.reload() };
}

/** Check if any applied change with frontend files was resolved after the page loaded.
 *  This replaces the backend's `client_update_available` flag which checks "since engine
 *  start" and incorrectly persists across page refreshes. */
export function hasClientUpdateSincePageLoad(applied: Change[]): boolean {
  return applied.some(c =>
    c.resolved_at && new Date(c.resolved_at).getTime() > PAGE_LOADED_AT &&
    c.files.some(f => CLIENT_FILE_RE.test(f))
  );
}

export const RESTART_GROUPS_LS_KEY = 'lucidos-restart-groups';
const LEGACY_RESTART_REASONS_LS_KEY = 'lucidos-restart-reasons';

/** Record an applied change as part of a thread group, mark the engine as
 *  needing a restart, and refresh the toast. Merges into the existing group
 *  for `threadId` (concat new commits, dedupe, refresh title) or appends.
 *  Empty commits are kept so the user still sees that the thread contributed. */
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
  syncRestartToast();
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

/** Toast text shown while a restart is pending. The detailed per-thread
 *  change list lives in the Restart confirm dialog (ControlPanel) — the
 *  toast is intentionally short to avoid overwhelming the chat view on
 *  long sessions. */
const RESTART_TOAST_MESSAGE = 'Engine restart required to apply changes.';

/** Set restarting state, show info toast, and call the restart API. */
export async function initiateEngineRestart(): Promise<void> {
  engineRestarting.value = true;
  // dismissable: false — see ToastItem.dismissable.
  showToast('Restarting engine...', 'info', { key: RESTART_TOAST_KEY, spinning: true, dismissable: false });
  try {
    await restartEngine();
  } catch (e) {
    if (e instanceof ApiError) {
      // Spawn-failure path: engine rejected restart and is still alive.
      // Revert the indicator so the UI doesn't freeze on "Restarting…".
      engineRestarting.value = false;
      showToast(`Restart failed: ${e.reason}`, 'error', { key: RESTART_TOAST_KEY });
      return;
    }
    // Network rejection after a 2xx: web-dev.sh is killing the engine.
    // Leave engineRestarting set; checkConnection() clears it on reconnect.
  }
}

function isEngineOutdated(): boolean {
  const ev = engineVersion.value;
  const lev = latestEngineVersion.value;
  return !!(ev && lev && isNewerVersion(lev, ev));
}

/** Stable fingerprint of the per-thread commits the toast is warning about.
 *  When the user adds a commit, the fingerprint changes and the previously
 *  dismissed toast comes back.
 *
 *  Engine-outdated state is intentionally excluded: the engineVersion /
 *  latestEngineVersion signals are populated asynchronously (after the
 *  health check resolves), so they're null at restoreRestartToast() time.
 *  Including them in the fingerprint would make the restored fingerprint
 *  spuriously diverge from the dismissed one and silently invalidate the
 *  dismissal across reloads. The ControlPanel badge still reflects engine
 *  outdated; the toast just doesn't re-nag on its own. */
function currentRestartFingerprint(): string {
  const groups = restartGroups.value
    .slice()
    .sort((a, b) => a.threadId.localeCompare(b.threadId))
    .map(g => [g.threadId, g.commits] as const);
  return JSON.stringify(groups);
}

/** User clicked Dismiss on the restart toast: hide it for the *current* set
 *  of restart-needing changes. `restartRequired` stays true so the
 *  ControlPanel "Restart needed" indicator remains visible. */
export function dismissRestartToast(): void {
  localStorage.setItem(RESTART_DISMISSED_FP_LS_KEY, currentRestartFingerprint());
  dismissToast(RESTART_TOAST_KEY);
}

/** Show or dismiss the restart-required toast based on restartRequired signal.
 *  Also persists to localStorage so the toast survives page reloads / Vite HMR.
 *  Honors a stored dismissal fingerprint: if the user dismissed the toast and
 *  the change set hasn't grown, stays hidden; otherwise the dismissal is
 *  cleared and the toast reappears. */
export function syncRestartToast(): void {
  if (restartRequired.value) {
    localStorage.setItem(RESTART_LS_KEY, 'true');
    const dismissedFp = localStorage.getItem(RESTART_DISMISSED_FP_LS_KEY);
    const currentFp = currentRestartFingerprint();
    if (dismissedFp !== null && dismissedFp === currentFp) {
      dismissToast(RESTART_TOAST_KEY);
      return;
    }
    if (dismissedFp !== null) localStorage.removeItem(RESTART_DISMISSED_FP_LS_KEY);
    // If warning toast already exists with same message, skip to avoid re-renders
    const existing = toasts.value.find(t => t.key === RESTART_TOAST_KEY && t.type === 'warning');
    if (existing && existing.message === RESTART_TOAST_MESSAGE) return;
    showToast(RESTART_TOAST_MESSAGE, 'warning', {
      key: RESTART_TOAST_KEY,
      action: { label: 'Restart', onClick: () => initiateEngineRestart() },
      secondaryAction: { label: 'Dismiss', onClick: () => dismissRestartToast(), variant: 'danger' },
    });
  } else {
    localStorage.removeItem(RESTART_LS_KEY);
    if (localStorage.getItem(RESTART_DISMISSED_FP_LS_KEY) !== null) {
      localStorage.removeItem(RESTART_DISMISSED_FP_LS_KEY);
    }
    restartGroups.value = [];
    persistRestartGroups();
    dismissToast(RESTART_TOAST_KEY);
  }
}

/** Restore the restart-required toast from localStorage on startup.
 *  Called before the async refreshChangesState() so the toast is visible
 *  immediately, even if the API call is slow or fails. */
export function restoreRestartToast(): void {
  localStorage.removeItem(LEGACY_RESTART_REASONS_LS_KEY);
  if (localStorage.getItem(RESTART_LS_KEY) === 'true') {
    try {
      const saved = localStorage.getItem(RESTART_GROUPS_LS_KEY);
      if (saved) restartGroups.value = JSON.parse(saved) as RestartGroup[];
    } catch {
      localStorage.removeItem(RESTART_GROUPS_LS_KEY);
    }
    restartRequired.value = true;
    syncRestartToast();
  }
}

/** Fetch changes from backend and update all related signals.
 *  On AbortError (10s client timeout) we retry once before bothering the user:
 *  the iOS PWA fires this from `runResumeSync` after every visibilitychange,
 *  and the cellular/Wi-Fi radio just-waking case can hang the first request
 *  past the timeout even when the engine is responding fast. */
export function refreshChangesState(): void {
  apiFetchChanges({ limit: 15 })
    .catch(e => {
      if (e instanceof DOMException && e.name === 'AbortError') {
        return apiFetchChanges({ limit: 15 });
      }
      throw e;
    })
    .then(state => {
      const applied = state.applied || [];
      changes.value = state.pending;
      appliedChanges.value = applied;
      changesHasMore.value = state.has_more_applied;
      restartRequired.value = state.restart_required || isEngineOutdated();
      // Backend is the source of truth across page reloads — the live
      // ChangeApplied SSE event isn't replayed, so the toast detail would
      // otherwise be lost.
      restartGroups.value = (state.restart_groups ?? []).map(g => ({
        threadId: g.thread_id ?? '',
        threadTitle: g.thread_title ?? 'unknown',
        commits: g.commits,
      }));
      if (hasClientUpdateSincePageLoad(applied)) updateAvailable.value = true;
      syncRestartToast();
    })
    .catch(e => showToast(`Failed to fetch changes: ${errorDetail(e)}`, 'error'));
}

/** Apply a single pending change by ID. */
export async function applySingleChange(id: string): Promise<void> {
  try {
    const result = await apiApply(id);
    if (result.status === 'conflict') {
      // Backend spawned a CC session in conflict_thread_id to resolve;
      // applyingChangeIds gets set via the MergeConflictDetected SSE event.
      const conflictThreadId = result.conflict_thread_id;
      const title = conflictThreadId ? threadMap.value.get(conflictThreadId)?.meta.title : undefined;
      const threadLabel = title && title !== PENDING_TITLE_PLACEHOLDER ? `“${title}”` : 'thread';
      showToast(`Merge conflict in ${threadLabel} — resolving automatically.`, 'warning', {
        onClick: conflictThreadId ? () => focusThread(conflictThreadId) : undefined,
      });
    } else if (result.status === 'hardening') {
      // Hardening recovery: backend spawned a hardening session that will auto-apply.
      // Track the change as "applying" so ChangesPanel shows persistent state.
      applyingChangeIds.value = new Set([...applyingChangeIds.value, id]);
      showToast('Hardening in progress — change will apply automatically after hardening.', 'info');
    }
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to apply change', 'error');
  }
}

/** Discard a single pending change by ID. */
export async function discardSingleChange(id: string): Promise<void> {
  try {
    await apiDiscard(id);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to discard change', 'error');
  }
}

/** Apply all changes. */
export async function applyAllChanges(): Promise<void> {
  try {
    await apiApplyAll();
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to apply changes', 'error');
  }
}

/** Discard all changes. */
export async function discardAllChanges(): Promise<void> {
  try {
    const result = await apiDiscardAll();
    if (result.failed > 0) {
      showToast(`${result.failed} change(s) failed to discard: ${result.errors.join('; ')}`, 'error');
    }
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to discard changes', 'error');
  }
}

/** Revert a previously applied change. */
export async function revertChange(id: string): Promise<void> {
  try {
    await apiRevert(id);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to revert change', 'error');
  }
}

function setLazyChange(id: string, value: Loadable<Change>): void {
  const next = new Map(lazyChanges.value);
  next.set(id, value);
  lazyChanges.value = next;
}

/** Fetch a `Change` row on demand when its id falls outside the
 *  `changes`/`appliedChanges` windows. No-op if already cached, in flight, or
 *  known unfetchable — the `loading` and `failed` entries in `lazyChanges`
 *  serve as the dedup token and negative cache respectively. */
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

/** Load the next page of applied changes (infinite scroll). */
export async function loadMoreChanges(): Promise<void> {
  if (changesLoadingMore.value || !changesHasMore.value) return;

  const current = appliedChanges.value;
  if (current.length === 0) return;

  const lastItem = current[current.length - 1];
  const resolvedAt = lastItem.resolved_at;
  if (!resolvedAt) return;
  const beforeTs = new Date(resolvedAt).getTime() / 1000;

  changesLoadingMore.value = true;
  try {
    const data = await apiFetchChanges({ limit: 15, before: beforeTs });
    appliedChanges.value = [...current, ...(data.applied || [])];
    changesHasMore.value = data.has_more_applied;
  } catch (e) {
    showToast(`Failed to load more changes: ${errorDetail(e)}`, 'error');
  } finally {
    changesLoadingMore.value = false;
  }
}
