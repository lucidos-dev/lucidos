import {
  repoSource, repoFiles, repoDiff, repoPending,
  repoViewMode, repoExpandedFolders, selectedLines,
  repoSelectedChangeId, repoChanges, repoChangesLoadingMore,
  activeMenuItem, repositories, showToast,
  panelOverlay, parseRepoPath, encodeRepoPath, SELECTED_CHANGE_KEY,
  threadMap,
} from '../store';
import { listRepoFiles, getChangeDiff, getChangeById, getRepoChanges, getThreadCcDiff, ApiError } from '../../api/client';
import type { Change, ThreadCcDiff } from '../../api/client';
import { toFailed, loadedOr, setLoadingIfFresh } from '../types';
import { revealContentPane } from './pane';
import { loadRepositories } from './chat';
import { pushNavState, replaceNavState } from './navigation';
import { errorDetail } from '../../utils/errorDetail';
import { appIdFromFolder } from '../../utils/appIdFromFolder';

export async function switchRepoSource(repoId: string | null): Promise<void> {
  repoSource.value = repoId;
  selectedLines.value = null;
  repoExpandedFolders.value = new Set();
  repoSelectedChangeId.value = null;

  repoViewMode.value = 'all';
  repoDiff.value = { status: 'not-loaded' };
  repoPending.value = null;
  repoFiles.value = { status: 'not-loaded' };
  repoChanges.value = { status: 'not-loaded' };

  if (!repoId) return;

  // Default view shows the repo at HEAD; selectRepoChange swaps to a CC branch
  // ref only when the user explicitly picks a change.
  await Promise.all([
    loadRepoFiles(repoId),
    loadRepoChanges(repoId),
  ]);
}

export async function loadRepoFiles(repoId: string): Promise<void> {
  // Always flip to loading: callers (selectRepoChange, viewThreadCcDiff)
  // change repoPending.branch_name before calling, so the previous file
  // tree is for a different ref and would be misleading if left visible.
  repoFiles.value = { status: 'loading' };
  try {
    const gitRef = repoPending.value?.branch_name;
    const files = await listRepoFiles(repoId, gitRef);
    repoFiles.value = { status: 'loaded', data: files };
  } catch (e: unknown) {
    repoFiles.value = toFailed(e);
  }
}

export function toggleRepoFolder(path: string): void {
  const next = new Set(repoExpandedFolders.value);
  if (next.has(path)) {
    next.delete(path);
  } else {
    next.add(path);
  }
  repoExpandedFolders.value = next;
}

export function expandAllRepoFolders(): void {
  if (repoFiles.value.status !== 'loaded') return;
  const folders = new Set<string>();
  for (const path of repoFiles.value.data) {
    const parts = path.split('/');
    for (let i = 1; i < parts.length; i++) {
      folders.add(parts.slice(0, i).join('/'));
    }
  }
  repoExpandedFolders.value = folders;
}

export function collapseAllRepoFolders(): void {
  repoExpandedFolders.value = new Set();
}

/** repoFiles is cached; reload it alongside repoChanges so a merge to main
 *  shows up in the Files tree without a manual refresh. */
export async function refreshRepoView(repoId: string): Promise<void> {
  await Promise.all([loadRepoFiles(repoId), loadRepoChanges(repoId)]);
}

export async function loadRepoChanges(repoId: string): Promise<void> {
  setLoadingIfFresh(repoChanges);
  try {
    const data = await getRepoChanges(repoId, 20);
    repoChanges.value = { status: 'loaded', data };
  } catch (e: unknown) {
    repoChanges.value = toFailed(e);
  }
}

export async function loadMoreRepoChanges(): Promise<void> {
  if (repoChangesLoadingMore.value) return;
  const current = repoChanges.value;
  if (current.status !== 'loaded' || !current.data.has_more) return;
  const repoId = repoSource.value;
  if (!repoId) return;

  const lastApplied = current.data.applied[current.data.applied.length - 1];
  if (!lastApplied?.resolved_at) return;

  repoChangesLoadingMore.value = true;
  try {
    const before = new Date(lastApplied.resolved_at).getTime() / 1000;
    const more = await getRepoChanges(repoId, 20, before);
    repoChanges.value = {
      status: 'loaded',
      data: {
        pending: current.data.pending,
        applied: [...current.data.applied, ...more.applied],
        has_more: more.has_more,
      },
    };
  } catch (e: unknown) {
    showToast(`Failed to load more changes: ${errorDetail(e)}`, 'error');
  } finally {
    repoChangesLoadingMore.value = false;
  }
}

export async function selectRepoChange(change: Change | null): Promise<void> {
  repoSelectedChangeId.value = change?.id ?? null;

  if (!change) {
    repoDiff.value = { status: 'not-loaded' };
    repoPending.value = null;
    repoViewMode.value = 'all';
    const repoId = repoSource.value;
    if (repoId) await loadRepoFiles(repoId);
    return;
  }

  // Set pending info before loading so loadRepoFiles uses the right git ref
  if (change.status === 'pending') {
    repoPending.value = {
      branch_name: change.branch_name,
      files: change.files,
      description: change.description,
      thread_id: change.thread_id,
    };
  } else {
    repoPending.value = null;
  }

  repoViewMode.value = 'changes';
  repoDiff.value = { status: 'loading' };

  // Load diff and files in parallel
  const repoId = repoSource.value;
  const diffPromise = getChangeDiff(change.id)
    .then(diff => { repoDiff.value = { status: 'loaded', data: diff }; })
    .catch((e: unknown) => { repoDiff.value = toFailed(e); });

  await Promise.all([
    diffPromise,
    repoId ? loadRepoFiles(repoId) : Promise.resolve(),
  ]);
}

export async function viewChangeDiffById(changeId: string): Promise<void> {
  try {
    const change = await getChangeById(changeId);
    await viewChangeDiff(change);
  } catch (e) {
    showToast(`Failed to load change: ${errorDetail(e)}`, 'error');
  }
}

export async function viewChangeDiff(change: Change): Promise<void> {
  activeMenuItem.value = 'files';
  panelOverlay.value = null;
  revealContentPane();
  await loadChangeContext(change);
  pushNavState();
}

/** Load the repo + diff state for a change without touching navigation/overlay.
 *  Used to restore diff context after a reload, when the panel overlay was
 *  re-hydrated from nav history but its repoDiff/repoSource backing state was lost. */
export async function loadChangeContext(change: Change): Promise<void> {
  if (repositories.value.status !== 'loaded') await loadRepositories();
  if (repositories.value.status === 'failed') {
    showToast('Failed to load repositories', 'error');
    return;
  }
  const repos = loadedOr(repositories.value, []);
  const repo = repos.find(r => r.path === change.repo_root);
  if (!repo) return;
  if (repoSource.value !== repo.id) await switchRepoSource(repo.id);
  await selectRepoChange(change);
}

export async function loadChangeContextById(changeId: string): Promise<void> {
  try {
    const change = await getChangeById(changeId);
    await loadChangeContext(change);
  } catch (e) {
    showToast(`Failed to restore diff: ${errorDetail(e)}`, 'error');
  }
}

/** Restore the user's last-selected change after a reload. Without this, the
 *  Files panel always lands on the All Files tree even if the user was
 *  viewing a Diff. Only definitively-gone IDs (404 from the engine) drop the
 *  saved id; transient failures (network, 5xx) keep it so the next reload
 *  retries — silently nuking on transient errors would lose the user's
 *  selection because of a momentary outage. */
export async function restoreRepoSelectionFromStorage(): Promise<void> {
  const savedId = localStorage.getItem(SELECTED_CHANGE_KEY);
  if (!savedId) return;
  // If a file-preview overlay re-hydrated for the same change, RepoFilePreview's
  // useEffect already calls loadChangeContextById — skip to avoid a duplicate
  // round-trip for getChangeById/getChangeDiff/listRepoFiles on every reload.
  const overlay = panelOverlay.value;
  if (overlay?.type === 'file-preview') {
    const parsed = parseRepoPath(overlay.path);
    if (parsed?.mode === 'diff' && parsed.changeId === savedId) return;
  }
  try {
    const change = await getChangeById(savedId);
    await loadChangeContext(change);
  } catch (e) {
    if (e instanceof ApiError && e.httpCode === 404) {
      localStorage.removeItem(SELECTED_CHANGE_KEY);
    }
    // Transient errors (network, 5xx): keep the saved id and let next reload retry.
  }
}

/** Open the file-preview panel on a path in the current repo. The split-view
 *  sidebar inside the panel reuses this — when called while the panel is
 *  already on screen, overwrite the existing nav slot so the unified panel is
 *  one history entry with the latest file selection winning, instead of
 *  stacking one entry per file the user clicks through. */
export function openRepoFilePreview(path: string, mode: 'file' | 'diff'): void {
  const repoId = repoSource.value;
  if (!repoId) return;
  // Snapshot before mutating panelOverlay so the push-vs-replace decision
  // sees the *previous* overlay type.
  const replaceInPlace = panelOverlay.value?.type === 'file-preview';
  const changeId = mode === 'diff' ? repoSelectedChangeId.value ?? undefined : undefined;
  selectedLines.value = null;
  panelOverlay.value = {
    type: 'file-preview',
    path: encodeRepoPath(repoId, mode, path, changeId),
  };
  revealContentPane();
  if (replaceInPlace) replaceNavState();
  else pushNavState();
}

/** Open the Files panel on the 3-dot diff of a CC worktree's branch.
 *
 *  Three flavors of CC threads land here:
 *  - **External-repo / Lucidos-source**: the worktree's git root maps to a
 *    registered `Repository` row. Bind to that repo and load file tree at the
 *    branch ref alongside the diff.
 *  - **App coding-agent**: the worktree's git root is the *workspace* (not a
 *    registered repo). Skip the registry lookup, stub `repoFiles` to empty
 *    (the "All Files" tab is meaningless without a registered repo), and pump
 *    the diff straight into the Changes view. The backend already scopes the
 *    response to `data/apps/<id>/`. */
export async function viewThreadCcDiff(threadId: string): Promise<void> {
  activeMenuItem.value = 'files';
  panelOverlay.value = null;
  revealContentPane();

  if (repositories.value.status !== 'loaded') await loadRepositories();
  if (repositories.value.status === 'failed') {
    showToast('Failed to load repositories', 'error');
    return;
  }

  // Flip to loading before the await so a stale prior diff doesn't leak
  // through to the panel during the network round-trip.
  repoDiff.value = { status: 'loading' };

  let diff: ThreadCcDiff;
  try {
    diff = await getThreadCcDiff(threadId);
  } catch (e) {
    repoDiff.value = toFailed(e);
    showToast(`Failed to load diff: ${errorDetail(e)}`, 'error');
    return;
  }

  const repos = loadedOr(repositories.value, []);
  const repo = repos.find(r => r.path === diff.repo_root);

  if (!repo) {
    const meta = threadMap.value.get(threadId)?.meta;
    if (meta?.codingAgentKind === 'app') {
      // App CC thread: no registered repo to bind to. The backend's response
      // is already scoped to data/apps/<id>/. Route through switchRepoSource
      // first to wipe any prior registered-repo state (repoExpandedFolders,
      // selectedLines, repoChanges) — then layer the app-CC diff over the
      // clean slate. Without the reset, lingering signals from a previous
      // repo session would still observe stale paths.
      const appId = appIdFromFolder(meta.codingAgentFolder);
      await switchRepoSource(null);
      repoPending.value = {
        branch_name: diff.branch_name,
        files: diff.files.map(f => f.path),
        description: appId
          ? `${diff.branch_name} vs ${diff.base_ref} — ${appId}`
          : `${diff.branch_name} vs ${diff.base_ref}`,
        thread_id: threadId,
      };
      repoViewMode.value = 'changes';
      repoDiff.value = { status: 'loaded', data: { files: diff.files } };
      pushNavState();
      return;
    }
    repoDiff.value = { status: 'not-loaded' };
    showToast(
      `Repo at ${diff.repo_root} is not registered — add it under Repositories to browse files`,
      'error',
    );
    return;
  }

  if (repoSource.value !== repo.id) await switchRepoSource(repo.id);

  repoSelectedChangeId.value = null;
  repoPending.value = {
    branch_name: diff.branch_name,
    files: diff.files.map(f => f.path),
    description: `${diff.branch_name} vs ${diff.base_ref}`,
    thread_id: threadId,
  };
  repoViewMode.value = 'changes';
  repoDiff.value = { status: 'loaded', data: { files: diff.files } };
  // Re-fetch file tree at the branch ref — switchRepoSource loaded it at HEAD
  // because repoPending was still null at that point.
  await loadRepoFiles(repo.id);
  pushNavState();
}
