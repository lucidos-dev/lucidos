import { showToast, dismissToast, changes, appliedChanges, changesHasMore, changesLoadingMore, restartRequired, updateAvailable, restartGroups, applyingChangeIds, toasts, engineRestarting, engineVersion, latestEngineVersion } from '../store';
import type { RestartGroup } from '../store';
import { applyChange as apiApply, discardChange as apiDiscard, applyAllChanges as apiApplyAll, discardAllChanges as apiDiscardAll, revertChange as apiRevert, fetchChanges as apiFetchChanges, restartEngine } from '../../api/client';
import { isNewerVersion } from '../../utils/version';
import { errorDetail } from '../../utils/errorDetail';
import type { Change } from '../../api/client';

const RESTART_TOAST_KEY = 'restart-required';
export const RESTART_LS_KEY = 'cognos-restart-required';

/** Timestamp (ms) when this page's JavaScript was loaded.
 *  Changes applied before this time are already reflected in the running client. */
const PAGE_LOADED_AT = Date.now();

const CLIENT_FILE_RE = /\.(ts|tsx|css|html|js|jsx)$/;

/** Check if any applied change with frontend files was resolved after the page loaded.
 *  This replaces the backend's `client_update_available` flag which checks "since engine
 *  start" and incorrectly persists across page refreshes. */
export function hasClientUpdateSincePageLoad(applied: Change[]): boolean {
  return applied.some(c =>
    c.resolved_at && new Date(c.resolved_at).getTime() > PAGE_LOADED_AT &&
    c.files.some(f => CLIENT_FILE_RE.test(f))
  );
}

export const RESTART_GROUPS_LS_KEY = 'cognos-restart-groups';
const LEGACY_RESTART_REASONS_LS_KEY = 'cognos-restart-reasons';

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
  showToast('Restarting engine...', 'info', { key: RESTART_TOAST_KEY, spinning: true });
  try {
    await restartEngine();
  } catch {
    // Engine will be killed by web-dev.sh — fetch may fail.
    // Don't reset engineRestarting here — the engine IS restarting,
    // checkConnection() will clear it when the engine comes back.
  }
}

/** Show or dismiss the restart-required toast based on restartRequired signal.
 *  Also persists to localStorage so the toast survives page reloads / Vite HMR. */
export function syncRestartToast(): void {
  if (restartRequired.value) {
    localStorage.setItem(RESTART_LS_KEY, 'true');
    // If warning toast already exists with same message, skip to avoid re-renders
    const existing = toasts.value.find(t => t.key === RESTART_TOAST_KEY && t.type === 'warning');
    if (existing && existing.message === RESTART_TOAST_MESSAGE) return;
    showToast(RESTART_TOAST_MESSAGE, 'warning', {
      key: RESTART_TOAST_KEY,
      action: { label: 'Restart', onClick: () => initiateEngineRestart() },
    });
  } else {
    localStorage.removeItem(RESTART_LS_KEY);
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

/** Fetch changes from backend and update all related signals. */
export function refreshChangesState(): void {
  apiFetchChanges({ limit: 15 }).then(state => {
    const applied = state.applied || [];
    changes.value = state.pending;
    appliedChanges.value = applied;
    changesHasMore.value = state.has_more_applied;
    const ev = engineVersion.value;
    const lev = latestEngineVersion.value;
    const engineOutdated = !!(ev && lev && isNewerVersion(lev, ev));
    restartRequired.value = state.restart_required || engineOutdated;
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
  }).catch(e => console.error('[Changes] Failed to fetch changes:', e));
}

/** Apply a single pending change by ID. */
export async function applySingleChange(id: string): Promise<void> {
  try {
    const result = await apiApply(id);
    if (result.status === 'conflict') {
      // Merge conflict — CC session spawned to resolve it.
      // applyingChangeIds is set via MergeConflictDetected SSE event.
      showToast('Merge conflict detected — resolving automatically.', 'warning');
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
