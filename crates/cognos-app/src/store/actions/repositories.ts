import {
  repoSource, repoFiles, repoDiff, repoPending,
  repoViewMode, repoExpandedFolders, selectedLines,
  repoSelectedChangeId, repoChanges, repoChangesLoadingMore,
  activeMenuItem, repositories, showToast,
  panelOverlay,
} from '../store';
import { listRepoFiles, getRepoDiff, getChangeDiff, getChangeById, getRepoChanges } from '../../api/client';
import type { Change } from '../../api/client';
import { toFailed, loadedOr } from '../types';
import { navigateToPane } from './pane';
import { isMobile } from '../../utils/viewport';
import { loadRepositories } from './chat';
import { pushNavState } from './navigation';
import { errorDetail } from '../../utils/errorDetail';

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
  repoFiles.value = { status: 'loading' };
  try {
    const gitRef = repoPending.value?.branch_name;
    const files = await listRepoFiles(repoId, gitRef);
    repoFiles.value = { status: 'loaded', data: files };
  } catch (e: unknown) {
    repoFiles.value = toFailed(e);
  }
}

export async function loadRepoDiff(repoId: string): Promise<void> {
  const branch = repoPending.value?.branch_name;
  if (!branch) {
    repoDiff.value = { status: 'not-loaded' };
    return;
  }
  repoDiff.value = { status: 'loading' };
  try {
    const diff = await getRepoDiff(repoId, branch);
    repoDiff.value = { status: 'loaded', data: diff };
  } catch (e: unknown) {
    repoDiff.value = toFailed(e);
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
  repoChanges.value = { status: 'loading' };
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
  if (repositories.value.status !== 'loaded') await loadRepositories();
  if (repositories.value.status === 'failed') {
    showToast('Failed to load repositories', 'error');
    return;
  }
  const repos = loadedOr(repositories.value, []);
  const repo = repos.find(r => r.path === change.repo_root);
  if (!repo) return;

  activeMenuItem.value = 'files';
  panelOverlay.value = null;
  if (isMobile()) navigateToPane('content');
  await switchRepoSource(repo.id);
  await selectRepoChange(change);
  pushNavState();
}
