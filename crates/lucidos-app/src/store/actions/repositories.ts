import {
  repoSource, repoFiles, repoDiff, repoPending,
  repoViewMode, repoExpandedFolders, selectedLines,
  repoSelectedChangeId, repoChanges, repoChangesLoadingMore,
  activeMenuItem, repositories, showToast,
  panelOverlay, parseRepoPath, encodeRepoPath, SELECTED_CHANGE_KEY,
  threadMap, type RepoDiff, type RepoLocator, type RepoPendingInfo, type Repository,
} from '../store';
import { listRepoFiles, getChangeDiff, getChangeById, getRepoChanges, getThreadCcDiff, ApiError } from '../../api/client';
import type { Change, ThreadCcDiff } from '../../api/client';
import { toFailed, failedIfFresh, loadedOr, setLoadingIfFresh, type Loadable } from '../types';
import { openFilePreview } from './artifacts';
import { revealContentPane } from './pane';
// From the module that DEFINES it, not chat.ts's back-compat re-export: the
// loader was split out of chat.ts precisely so a consumer needn't pull chat's
// transitive tree (connection, chat-changes, ...) in behind it.
import { loadRepositories } from './repositoriesLoader';
import { pushNavState, replaceNavState } from './navigation';
import { errorDetail } from '../../utils/errorDetail';
import { appIdFromFolder } from '../../utils/appIdFromFolder';

/** Bumped by every Files panel navigation. A diff load checks it after each
 *  await and stops writing once a newer navigation has taken the panel. */
let filesNavigation = 0;

export async function switchRepoSource(repoId: string | null): Promise<void> {
  filesNavigation++;
  bindRepoSource(repoId);
  repoSelectedChangeId.value = null;
  repoViewMode.value = 'all';
  repoDiff.value = { status: 'not-loaded' };
  repoPending.value = null;

  if (!repoId) return;

  // Default view shows the repo at HEAD; selectRepoChange swaps to a CC branch
  // ref only when the user explicitly picks a change.
  await Promise.all([
    loadRepoFiles(repoId),
    loadRepoChanges(repoId),
  ]);
}

/** Point the Files panel at a repo and drop the previous repo's state. Leaves
 *  the view mode, diff and change selection alone, so a diff navigation that
 *  staged them keeps them. */
function bindRepoSource(repoId: string | null): void {
  repoSource.value = repoId;
  selectedLines.value = null;
  repoExpandedFolders.value = new Set();
  repoFiles.value = { status: 'not-loaded' };
  repoChanges.value = { status: 'not-loaded' };
}

/** Put the Files panel into its diff view with the diff still loading. Every
 *  diff navigation calls this BEFORE its first await. The panel then renders
 *  its loading state rather than the All Files tree or the previous diff.
 *  Returns whether this navigation still owns the panel. */
function stageDiffView(changeId: string | null): () => boolean {
  const navigation = ++filesNavigation;
  repoSelectedChangeId.value = changeId;
  repoPending.value = null;
  repoViewMode.value = 'changes';
  repoDiff.value = { status: 'loading' };
  return () => navigation === filesNavigation;
}

/** Report a failed diff load. The panel shows it only while this navigation
 *  still owns the panel; the toast shows either way. */
function failStagedDiff(isCurrent: () => boolean, reason: string): void {
  showToast(reason, 'error');
  if (isCurrent()) repoDiff.value = { status: 'failed', error: reason };
}

/** Land on the staged diff view: one navigation, one history entry. */
function landOnFilesPanel(): void {
  activeMenuItem.value = 'files';
  panelOverlay.value = null;
  revealContentPane();
  pushNavState();
}

export async function loadRepoFiles(repoId: string): Promise<void> {
  // Always flip to loading: callers (selectRepoChange, viewThreadCcDiff)
  // change repoPending.branch_name before calling, so the previous file
  // tree is for a different ref and would be misleading if left visible.
  repoFiles.value = { status: 'loading' };
  const gitRef = repoPending.value?.branch_name;
  // A tree for a repo or branch the panel has since left must not land.
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

/** repoFiles is cached; reload it alongside repoChanges so a merge to main
 *  shows up in the Files tree without a manual refresh. */
export async function refreshRepoView(repoId: string): Promise<void> {
  await Promise.all([loadRepoFiles(repoId), loadRepoChanges(repoId)]);
}

export async function loadRepoChanges(repoId: string): Promise<void> {
  setLoadingIfFresh(repoChanges);
  try {
    const data = await getRepoChanges(repoId, 20);
    // A list for a repo the panel has since left must not land.
    if (repoSource.value !== repoId) return;
    repoChanges.value = { status: 'loaded', data };
  } catch (e: unknown) {
    if (repoSource.value !== repoId) return;
    // `setLoadingIfFresh` above keeps a loaded list visible through the round
    // trip, so the failure path must match. A transient refetch failure (e.g.
    // `refreshRepoView` after a change applies) keeps the last-good list rather
    // than showing an error. Only a first load records failed.
    repoChanges.value = failedIfFresh(repoChanges.value, e);
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

  // Set pending info before loading so loadRepoFiles uses the right git ref
  repoPending.value = pendingFromChange(change);
  repoViewMode.value = 'changes';
  repoDiff.value = { status: 'loading' };

  // Load diff and files in parallel
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

/** The pending-branch info a change carries. Applied changes have none: they
 *  read at HEAD. */
function pendingFromChange(change: Change): RepoPendingInfo | null {
  if (change.status !== 'pending') return null;
  return {
    branch_name: change.branch_name,
    files: change.files,
    description: change.description,
    thread_id: change.thread_id,
  };
}

/** Open a change's diff: the file list, never a single file, even when the
 *  change touches only one. */
export async function viewChangeDiff(change: Change): Promise<void> {
  const isCurrent = stageDiffView(change.id);
  landOnFilesPanel();
  await loadStagedChangeDiff(change, isCurrent);
}

/** Load the registered repositories if needed. Returns why that failed, or
 *  null. Shared by the diff paths below. Carries the reason the Loadable
 *  already holds: a bare generic drops the last link of the error chain
 *  (.claude/rules/frontend.md, no hidden errors). */
async function ensureRepositoriesLoaded(): Promise<string | null> {
  if (repositories.value.status !== 'loaded') await loadRepositories();
  if (repositories.value.status !== 'failed') return null;
  return `Failed to load repositories: ${repositories.value.error}`;
}

/** The registered `Repository` whose root is `path`, or null.
 *
 *  A miss re-reads the registry before answering. `repositories` is a cached
 *  projection refreshed by `Repository*` SSE. A repo registered moments ago by
 *  a sibling thread, by `manage_repositories`, or at engine startup therefore
 *  leaves it stale. Answering "no" off that cache reports a live repo as
 *  unregistered, which `.claude/rules/frontend.md` forbids. Mirrors
 *  `navigateToTrigger`.
 *
 *  The re-read hands the snapshot back if it fails. A blip on this second read
 *  must not turn a loaded registry into `failed` for every other surface. */
async function findRegisteredRepo(path: string): Promise<Repository | null> {
  const hit = loadedOr(repositories.value, []).find(r => r.path === path);
  if (hit) return hit;
  const snapshot = repositories.value;
  await loadRepositories();
  if (repositories.value.status === 'failed') repositories.value = snapshot;
  return loadedOr(repositories.value, []).find(r => r.path === path) ?? null;
}

/** Load the repo + diff state for a change without touching navigation/overlay.
 *  Used to restore diff context after a reload, when the panel overlay was
 *  re-hydrated from nav history but its repoDiff/repoSource backing state was lost. */
export async function loadChangeContext(change: Change): Promise<void> {
  await loadStagedChangeDiff(change, stageDiffView(change.id));
}

/** Resolve a staged change's repo and fill in its diff. The diff lands together
 *  with a newly bound repo's change list, so the panel header and the file list
 *  appear in one step. Work for a change the user has since left is dropped. */
async function loadStagedChangeDiff(change: Change, isCurrent: () => boolean): Promise<void> {
  const reposFailed = await ensureRepositoriesLoaded();
  if (reposFailed) { failStagedDiff(isCurrent, reposFailed); return; }
  const repo = await findRegisteredRepo(change.repo_root);
  if (!isCurrent()) return;
  if (!repo) {
    // No registered Repository matches change.repo_root: app coding-agent
    // changes use the workspace root, and a change whose repo was later removed
    // has no row either. Render the change's diff inline rather than bailing.
    await loadUnregisteredChangeDiff(change, isCurrent);
    return;
  }
  const rebound = repoSource.value !== repo.id;
  if (rebound) bindRepoSource(repo.id);
  // Before loadRepoFiles, which reads the tree at the pending branch.
  repoPending.value = pendingFromChange(change);
  // The tree serves only All Files, so the diff does not wait on it.
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

/** Render a change's diff inline when no registered repo backs its repo_root:
 *  app coding-agent changes (repo_root = workspace root) and changes whose repo
 *  was later removed. Mirrors viewThreadCcDiff's app branch: the backend already
 *  scopes app changes to data/apps/<id>/, and the "All Files" tab is meaningless
 *  without a registered repo, but the diff itself is not. Unbind any prior
 *  registered repo first so lingering signals don't observe stale paths. */
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

  // Best-effort app-id label, mirroring viewThreadCcDiff — only when the change's
  // thread is loaded AND is an app coding-agent thread. Absent (e.g. the Changes
  // panel showing a change for an unloaded thread) → just the change description.
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
  // No `ref` on the file locator: the panel is bound to one repository, so
  // `RepoFilePreview` reads its files at that repository's pending coding-agent
  // branch. Naming a ref here is for callers from OUTSIDE the panel, which have
  // no such binding to fall back on.
  const locator: RepoLocator = mode === 'diff'
    ? { repoId, mode, changeId: repoSelectedChangeId.value ?? undefined, path }
    : { repoId, mode, path };
  selectedLines.value = null;
  panelOverlay.value = { type: 'file-preview', path: encodeRepoPath(locator) };
  revealContentPane();
  if (replaceInPlace) replaceNavState();
  else pushNavState();
}

/** Open an ALREADY-encoded repo preview path (`repo:<repoId>:file:<path>`, the
 *  form `encodeRepoPath` produces) that arrived from OUTSIDE the Files panel —
 *  an app iframe's `lucidos.ui.navigate('file', …)` or an engine
 *  `NavigationRequested`. Returns false when the path isn't repo-encoded, so the
 *  caller falls back to the workspace-data file preview.
 *
 *  `openRepoFilePreview` above is the in-panel sibling: it ENCODES a path against
 *  the repo the user is already browsing. Here the path arrives encoded and the
 *  caller has no repo context, so bind the repo first. The preview itself takes
 *  repoId/path as props and renders with no repo state at all (that's what makes
 *  a reload of a restored repo preview work) — but the panel's split sidebar
 *  lists whatever `repoDiff` holds, and its rows re-open through
 *  `openRepoFilePreview`, which targets `repoSource`. Landing on repo A while
 *  repo B's diff is still loaded would list B's changed files beside A's file and
 *  open B's on click. `switchRepoSource` wipes repoDiff/repoPending/repoFiles
 *  synchronously, so the stale sidebar is gone before the overlay mounts.
 *  Already on that repo → keep the user's change selection (mirrors
 *  `loadChangeContext`). Either way the line selection is dropped, so a prior
 *  file's highlighted range can't leak onto this one; that clearing lives in
 *  `openFilePreview` below, which every path out of here goes through. */
export function openEncodedRepoFilePreview(encoded: string): boolean {
  const parsed = parseRepoPath(encoded);
  if (!parsed) return false;
  if (repoSource.value !== parsed.repoId) void switchRepoSource(parsed.repoId);
  openFilePreview(encoded);
  return true;
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
  // A thread diff has no Change row, so no change id.
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
      // App CC thread: no registered repo to bind to. The backend's response
      // is already scoped to data/apps/<id>/. Unbind any prior registered repo
      // first, so lingering signals from it don't observe stale paths.
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
  // Before loadRepoFiles, which reads the tree at the pending branch.
  repoPending.value = {
    branch_name: diff.branch_name,
    files: diff.files.map(f => f.path),
    description: `${diff.branch_name} vs ${diff.base_ref}`,
    thread_id: threadId,
  };
  // The tree serves only All Files, so the diff does not wait on it. A newly
  // bound repo's change list lands with the diff, so the header and the file
  // list appear in one step.
  const files = loadRepoFiles(repo.id);
  if (rebound) await loadRepoChanges(repo.id);
  if (isCurrent()) repoDiff.value = { status: 'loaded', data: { files: diff.files } };
  await files;
}
