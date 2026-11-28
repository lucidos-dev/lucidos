import {
  repoSource, repoFiles, repoDiff, repoPending,
  repoViewMode, repoExpandedFolders, expandedFolders, revealedFolder, selectedLines, lineScrollTarget, findRequest,
  repoSelectedChangeId, repoChanges, repoChangesLoadingMore,
  repositories, showToast,
  panelOverlay, parseRepoPath, encodeRepoPath,
  threadMap, type RepoDiff, type RepoLocator, type RepoPendingInfo, type Repository,
} from '../store';
import { listRepoFiles, getChangeDiff, getChangeById, getRepoChanges, getThreadCcDiff } from '../../api/client';
import type { Change, ThreadCcDiff } from '../../api/client';
import { toFailed, failedIfFresh, loadedOr, setLoadingIfFresh, type Loadable } from '../types';
import { openFilePreview } from './artifacts';
import { revealContentPane } from './pane';
import { setActiveMenu } from './menu';
import { loadRepositories } from './repositoriesLoader';
import { pushNavState, replaceNavState } from './navigation';
import { errorDetail } from '../../utils/errorDetail';
import { appIdFromFolder } from '../../utils/appIdFromFolder';
import { resetContentScroll } from '../savedScroll';

let filesNavigation = 0;

export async function switchRepoSource(repoId: string | null): Promise<void> {
  filesNavigation++;
  bindRepoSource(repoId);
  repoSelectedChangeId.value = null;
  repoViewMode.value = 'all';
  repoDiff.value = { status: 'not-loaded' };
  repoPending.value = null;

  if (!repoId) return;

  await Promise.all([
    loadRepoFiles(repoId),
    loadRepoChanges(repoId),
  ]);
}

function bindRepoSource(repoId: string | null): void {
  repoSource.value = repoId;
  selectedLines.value = null;
  repoExpandedFolders.value = new Set();
  revealedFolder.value = null;
  repoFiles.value = { status: 'not-loaded' };
  repoChanges.value = { status: 'not-loaded' };
}

function stageDiffView(changeId: string | null): () => boolean {
  const navigation = ++filesNavigation;
  repoSelectedChangeId.value = changeId;
  repoPending.value = null;
  repoViewMode.value = 'changes';
  repoDiff.value = { status: 'loading' };
  return () => navigation === filesNavigation;
}

function failStagedDiff(isCurrent: () => boolean, reason: string): void {
  showToast(reason, 'error');
  if (isCurrent()) repoDiff.value = { status: 'failed', error: reason };
}

function landOnFilesPanel(): void {
  setActiveMenu('files');
  revealContentPane();
  pushNavState();
}

export function revealFolderInFiles(locator: string, folder: string): void {
  const repoId = parseRepoPath(locator)?.repoId ?? null;
  if (repoSource.value !== repoId) {
    void switchRepoSource(repoId);
  } else {
    repoViewMode.value = 'all';
  }

  const expanded = repoId ? repoExpandedFolders : expandedFolders;
  const next = new Set(expanded.value);
  const parts = folder.split('/');
  for (let i = 1; i <= parts.length; i++) next.add(parts.slice(0, i).join('/'));
  expanded.value = next;

  revealedFolder.value = folder;
  resetContentScroll('files');
  landOnFilesPanel();
}

export async function loadRepoFiles(repoId: string, opts: { reread?: boolean } = {}): Promise<void> {
  if (opts.reread) setLoadingIfFresh(repoFiles);
  else repoFiles.value = { status: 'loading' };
  const gitRef = repoPending.value?.branch_name;
  const stillWanted = () => repoSource.value === repoId && repoPending.value?.branch_name === gitRef;
  try {
    const files = await listRepoFiles(repoId, gitRef);
    if (stillWanted()) repoFiles.value = { status: 'loaded', data: files };
  } catch (e: unknown) {
    if (stillWanted()) repoFiles.value = toFailed(e);
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

export async function refreshRepoView(repoId: string): Promise<void> {
  await Promise.all([loadRepoFiles(repoId, { reread: true }), loadRepoChanges(repoId)]);
}

export async function loadRepoChanges(repoId: string): Promise<void> {
  setLoadingIfFresh(repoChanges);
  try {
    const data = await getRepoChanges(repoId, 20);
    if (repoSource.value !== repoId) return;
    repoChanges.value = { status: 'loaded', data };
  } catch (e: unknown) {
    if (repoSource.value !== repoId) return;
    repoChanges.value = failedIfFresh(repoChanges.value, e);
  }
}

const MAX_REPO_CHANGES_PAGE_RETRIES = 3;

export async function loadMoreRepoChanges(): Promise<void> {
  if (repoChangesLoadingMore.value) return;
  const repoId = repoSource.value;
  if (!repoId) return;

  repoChangesLoadingMore.value = true;
  try {
    for (let attempt = 0; attempt < MAX_REPO_CHANGES_PAGE_RETRIES; attempt++) {
      const base = repoChanges.value;
      if (base.status !== 'loaded' || !base.data.has_more) return;
      const lastApplied = base.data.applied[base.data.applied.length - 1];
      if (!lastApplied?.resolved_at) return;

      const before = new Date(lastApplied.resolved_at).getTime() / 1000;
      const more = await getRepoChanges(repoId, 20, before);

      if (repoSource.value !== repoId) return;
      const after = repoChanges.value;
      if (after.status !== 'loaded') return;
      if (after !== base) continue;
      const seen = new Set(after.data.applied.map((c) => c.id));
      repoChanges.value = {
        status: 'loaded',
        data: {
          pending: after.data.pending,
          applied: [...after.data.applied, ...more.applied.filter((c) => !seen.has(c.id))],
          has_more: more.has_more,
        },
      };
      return;
    }
  } catch (e: unknown) {
    showToast(`Failed to load more changes: ${errorDetail(e)}`, 'error');
  } finally {
    repoChangesLoadingMore.value = false;
  }
}

export async function selectRepoChange(change: Change | null): Promise<void> {
  const navigation = ++filesNavigation;
  repoSelectedChangeId.value = change?.id ?? null;

  if (!change) {
    repoDiff.value = { status: 'not-loaded' };
    repoPending.value = null;
    repoViewMode.value = 'all';
    const repoId = repoSource.value;
    if (repoId) await loadRepoFiles(repoId);
    return;
  }

  repoPending.value = pendingFromChange(change);
  repoViewMode.value = 'changes';
  repoDiff.value = { status: 'loading' };

  const repoId = repoSource.value;
  const isCurrent = () => navigation === filesNavigation;
  const diffPromise = getChangeDiff(change.id)
    .then(diff => { if (isCurrent()) repoDiff.value = { status: 'loaded', data: diff }; })
    .catch((e: unknown) => { if (isCurrent()) repoDiff.value = toFailed(e); });

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

function pendingFromChange(change: Change): RepoPendingInfo | null {
  if (change.status !== 'pending' && change.status !== 'set_aside') return null;
  return {
    branch_name: change.branch_name,
    files: change.files,
    description: change.description,
    thread_id: change.thread_id,
  };
}

export async function viewChangeDiff(change: Change): Promise<void> {
  const isCurrent = stageDiffView(change.id);
  landOnFilesPanel();
  await loadStagedChangeDiff(change, isCurrent);
}

async function ensureRepositoriesLoaded(): Promise<string | null> {
  if (repositories.value.status !== 'loaded') await loadRepositories();
  if (repositories.value.status !== 'failed') return null;
  return `Failed to load repositories: ${repositories.value.error}`;
}

async function findRegisteredRepo(path: string): Promise<Repository | null> {
  const hit = loadedOr(repositories.value, []).find(r => r.path === path);
  if (hit) return hit;
  const snapshot = repositories.value;
  await loadRepositories();
  if (repositories.value.status === 'failed') repositories.value = snapshot;
  return loadedOr(repositories.value, []).find(r => r.path === path) ?? null;
}

export async function loadChangeContext(change: Change): Promise<void> {
  await loadStagedChangeDiff(change, stageDiffView(change.id));
}

async function loadStagedChangeDiff(change: Change, isCurrent: () => boolean): Promise<void> {
  const reposFailed = await ensureRepositoriesLoaded();
  if (reposFailed) { failStagedDiff(isCurrent, reposFailed); return; }
  const repo = await findRegisteredRepo(change.repo_root);
  if (!isCurrent()) return;
  if (!repo) {
    await loadUnregisteredChangeDiff(change, isCurrent);
    return;
  }
  const rebound = repoSource.value !== repo.id;
  if (rebound) bindRepoSource(repo.id);
  repoPending.value = pendingFromChange(change);
  const files = loadRepoFiles(repo.id);
  const [diff] = await Promise.all([
    getChangeDiff(change.id).then(
      (data): Loadable<RepoDiff> => ({ status: 'loaded', data }),
      (e: unknown) => toFailed<RepoDiff>(e),
    ),
    rebound ? loadRepoChanges(repo.id) : undefined,
  ]);
  if (isCurrent()) repoDiff.value = diff;
  await files;
}

async function loadUnregisteredChangeDiff(change: Change, isCurrent: () => boolean): Promise<void> {
  bindRepoSource(null);

  let diff: RepoDiff;
  try {
    diff = await getChangeDiff(change.id);
  } catch (e) {
    failStagedDiff(isCurrent, `Failed to load diff: ${errorDetail(e)}`);
    return;
  }
  if (!isCurrent()) return;

  const meta = change.thread_id ? threadMap.value.get(change.thread_id)?.meta : undefined;
  const appId = meta?.codingAgentKind === 'app' ? appIdFromFolder(meta.codingAgentFolder) : null;

  repoPending.value = {
    branch_name: change.branch_name,
    files: diff.files.map(f => f.path),
    description: appId ? `${change.description} (${appId})` : change.description,
    thread_id: change.thread_id,
  };
  repoDiff.value = { status: 'loaded', data: diff };
}

export async function loadChangeContextById(changeId: string): Promise<void> {
  try {
    const change = await getChangeById(changeId);
    await loadChangeContext(change);
  } catch (e) {
    showToast(`Failed to restore diff: ${errorDetail(e)}`, 'error');
  }
}

export { restoreRepoSelectionFromStorage } from './repoSelectionRestore';

export function openRepoFilePreview(path: string, mode: 'file' | 'diff'): void {
  const repoId = repoSource.value;
  if (!repoId) return;
  const replaceInPlace = panelOverlay.value?.type === 'file-preview';
  const locator: RepoLocator = mode === 'diff'
    ? { repoId, mode, changeId: repoSelectedChangeId.value ?? undefined, path }
    : { repoId, mode, path };
  selectedLines.value = null;
  lineScrollTarget.value = null;
  findRequest.value = null;
  panelOverlay.value = { type: 'file-preview', path: encodeRepoPath(locator) };
  revealContentPane();
  if (replaceInPlace) replaceNavState();
  else pushNavState();
}

export function openEncodedRepoFilePreview(encoded: string): boolean {
  const parsed = parseRepoPath(encoded);
  if (!parsed) return false;
  if (repoSource.value !== parsed.repoId) void switchRepoSource(parsed.repoId);
  openFilePreview(encoded);
  return true;
}

export async function viewThreadCcDiff(threadId: string): Promise<void> {
  const isCurrent = stageDiffView(null);
  landOnFilesPanel();

  const reposFailed = await ensureRepositoriesLoaded();
  if (reposFailed) { failStagedDiff(isCurrent, reposFailed); return; }

  let diff: ThreadCcDiff;
  try {
    diff = await getThreadCcDiff(threadId);
  } catch (e) {
    failStagedDiff(isCurrent, `Failed to load diff: ${errorDetail(e)}`);
    return;
  }

  const repo = await findRegisteredRepo(diff.repo_root);
  if (!isCurrent()) return;

  if (!repo) {
    const meta = threadMap.value.get(threadId)?.meta;
    if (meta?.codingAgentKind === 'app') {
      const appId = appIdFromFolder(meta.codingAgentFolder);
      bindRepoSource(null);
      repoPending.value = {
        branch_name: diff.branch_name,
        files: diff.files.map(f => f.path),
        description: appId
          ? `${diff.branch_name} vs ${diff.base_ref} (${appId})`
          : `${diff.branch_name} vs ${diff.base_ref}`,
        thread_id: threadId,
      };
      repoDiff.value = { status: 'loaded', data: { files: diff.files } };
      return;
    }
    failStagedDiff(isCurrent, `Repo at ${diff.repo_root} is not registered. Add it under Repositories to browse files.`);
    return;
  }

  const rebound = repoSource.value !== repo.id;
  if (rebound) bindRepoSource(repo.id);
  repoPending.value = {
    branch_name: diff.branch_name,
    files: diff.files.map(f => f.path),
    description: `${diff.branch_name} vs ${diff.base_ref}`,
    thread_id: threadId,
  };
  const files = loadRepoFiles(repo.id);
  if (rebound) await loadRepoChanges(repo.id);
  if (isCurrent()) repoDiff.value = { status: 'loaded', data: { files: diff.files } };
  await files;
}
