import {
  repoFiles, repoDiff, repoViewMode, repoExpandedFolders,
  repoPending, repoSelectedChangeId, repoSource,
} from '../../store/store';
import type { DiffFile } from '../../store/store';
import {
  toggleRepoFolder, openRepoFilePreview,
} from '../../store/actions/repositories';
import { buildFolderTree } from '../../store/actions/artifacts';
import type { FolderNode } from '../../store/actions/artifacts';
import { useDelayedLoading, useDelayedFlag } from '../../hooks/useDelayedLoading';
import { loadedOr, type Loadable } from '../../store/types';
import { ListSkeletonOf } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { FileTypeIcon } from '../../utils/fileIcons';
import { TreeNode, folderTreeSkeletonRow } from './FolderTree';
import { changeBadgeLabel } from './changeBadge';
import { diffStats } from './diffStats';
import { DiffStatsInline, DiffView } from './DiffView';

/** Whether the registered-repo Files view has the data its CURRENT mode renders.
 *  Each mode gates ONLY on the data it actually draws, so the view is ready as
 *  soon as that data is in — not blocked on the other mode's fetch:
 *  - All-files mode draws the file tree (repoFiles) → gate on the tree.
 *  - Changes mode draws the diff (repoDiff), NOT the tree → gate on the diff.
 *  Gating Changes mode on the diff alone lets the diff list appear as soon as
 *  the diff loads, instead of waiting for the whole-repo tree listing that only
 *  All Files needs (and which is often the slower call). The diff gate still
 *  prevents the "No changes" flash — an empty [] rendered while the diff is in
 *  flight (and before the single-file overlay opens over it). Exported for the
 *  unit test. */
export function repoFilesContentReady(opts: {
  filesStatus: Loadable<unknown>['status'];
  diffStatus: Loadable<unknown>['status'];
  mode: 'all' | 'changes';
}): boolean {
  if (opts.mode === 'changes') return opts.diffStatus === 'loaded';
  return opts.filesStatus === 'loaded';
}

export function RepoFilesView() {
  const loadable = repoFiles.value;
  const diffLoadable = repoDiff.value;
  // App-CC branch shows the diff directly (no repoFiles tree), so its spinner
  // must key off the diff's own load state, not repoFiles'.
  const diffShowLoading = useDelayedLoading(diffLoadable);
  const pending = repoPending.value;
  const mode = repoViewMode.value;
  const diffLoaded = diffLoadable.status === 'loaded';
  const contentReady = repoFilesContentReady({
    filesStatus: loadable.status,
    diffStatus: diffLoadable.status,
    mode,
  });
  const showLoading = useDelayedFlag(!contentReady);
  // Without a registered repo (app coding-agent threads use the workspace
  // as their git root, which isn't a Repository row), the "All Files" tab
  // would render an empty tree — hide the toggle and pin Changes view.
  // The repo-file tree (repoFiles) isn't fetched for the app-CC path either,
  // so don't gate the render on its load state.
  const hasRepo = repoSource.value != null;

  // A file-tree listing failure only breaks All Files (it renders the tree);
  // Changes mode draws the diff and doesn't need the tree, so don't let a tree
  // failure hide a perfectly good diff. Switching to All Files surfaces it.
  if (hasRepo && mode === 'all' && loadable.status === 'failed') {
    return (
      <div class="files-toolbar">
        <span class="files-hint error-text">Failed to load: {loadable.error}</span>
      </div>
    );
  }

  if (diffLoadable.status === 'failed') {
    return (
      <div class="files-toolbar">
        <span class="files-hint error-text">Failed to load diff: {diffLoadable.error}</span>
      </div>
    );
  }

  const changedFiles = diffLoaded ? diffLoadable.data.files : [];
  const changedMap = new Map(changedFiles.map(f => [f.path, f]));

  // App-CC view: skip the toolbar entirely (no toggle / no expand-all
  // controls apply) and render the inline diff. The empty bordered toolbar
  // bar was a visual artifact.
  if (!hasRepo) {
    // A non-terminal diff (loading or not-loaded) must show the spinner, not
    // fall through to InlineDiffList([]) -> "No changes". viewThreadCcDiff sets
    // loading before this view mounts, but guard not-loaded defensively too.
    if (diffLoadable.status !== 'loaded') {
      return diffShowLoading ? <div class="loading-spinner" /> : null;
    }
    return (
      <div class="artifacts-desktop">
        <InlineDiffList files={changedFiles} />
      </div>
    );
  }

  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf fill containerClass="folder-tree" row={folderTreeSkeletonRow} />}>
      {contentReady ? (
        <>
          {/* Toolbar holds only the All Files / Changes toggle now (Expand/Collapse
              all were removed). Skip rendering it entirely when there's no change to
              toggle against, so we don't leave an empty padded bar. */}
          {(pending || repoSelectedChangeId.value) && (
            <div class="files-toolbar files-toolbar-bordered">
              <span class="files-toolbar-actions">
                <span class="repo-view-toggle">
                  <button
                    class={`files-toolbar-btn ${mode === 'all' ? 'active' : ''}`}
                    onClick={() => { repoViewMode.value = 'all'; }}
                  >
                    All Files
                  </button>
                  <button
                    class={`files-toolbar-btn ${mode === 'changes' ? 'active' : ''}`}
                    onClick={() => { repoViewMode.value = 'changes'; }}
                  >
                    Changes {diffLoaded ? `(${changedFiles.length})` : '…'}
                  </button>
                </span>
              </span>
            </div>
          )}
          <div class="artifacts-desktop">
            {mode === 'changes' ? (
              <ChangesFileList files={changedFiles} />
            ) : (
              <RepoFolderTree changedMap={changedMap} />
            )}
          </div>
        </>
      ) : null}
    </LoadingFade>
  );
}

export function ChangesFileList({ files, activePath }: { files: DiffFile[]; activePath?: string }) {
  let totalAdd = 0;
  let totalDel = 0;
  const fileStats = files.map(f => {
    const s = diffStats(f);
    totalAdd += s.additions;
    totalDel += s.deletions;
    return s;
  });

  return (
    <div class="folder-tree">
      {files.length > 0 && (
        <div class="diff-stats-total">
          <DiffStatsInline additions={totalAdd} deletions={totalDel} />
        </div>
      )}
      {files.map((f, i) => (
        <div
          key={f.path}
          class={`file-item repo-changed-file${f.path === activePath ? ' active' : ''}`}
          onClick={() => openRepoFilePreview(f.path, 'diff')}
        >
          <FileTypeIcon path={f.path} className="file-icon" />
          {/* `file-path`, not the tree's bare `file-name`: this row carries the
              whole path, which wraps rather than ellipsising (components.css). */}
          <span class="file-name file-path">{f.path}</span>
          <DiffStatsInline additions={fileStats[i].additions} deletions={fileStats[i].deletions} />
          <span class={`change-badge change-badge-${f.status}`}>
            {changeBadgeLabel(f.status)}
          </span>
        </div>
      ))}
      {files.length === 0 && (
        <div class="empty-state">No changes</div>
      )}
    </div>
  );
}

/** Diff renderer for app coding-agent threads (no registered repo to back the
 *  file-preview panel). Stacks each file's hunks inline so the user gets the
 *  full diff in one scroll, without depending on openRepoFilePreview (which
 *  no-ops when repoSource is null). */
export function InlineDiffList({ files }: { files: DiffFile[] }) {
  if (files.length === 0) {
    return <div class="empty-state">No changes</div>;
  }
  let totalAdd = 0;
  let totalDel = 0;
  for (const f of files) {
    const s = diffStats(f);
    totalAdd += s.additions;
    totalDel += s.deletions;
  }
  return (
    <div class="folder-tree">
      <div class="diff-stats-total">
        <DiffStatsInline additions={totalAdd} deletions={totalDel} />
      </div>
      {files.map(f => (
        <DiffView key={f.path} file={f} />
      ))}
    </div>
  );
}

function folderHasChanges(n: FolderNode, changedMap: Map<string, DiffFile>): boolean {
  for (const f of n.files) {
    if (changedMap.has(f.path)) return true;
  }
  for (const child of Object.values(n.children)) {
    if (folderHasChanges(child, changedMap)) return true;
  }
  return false;
}

function RepoFolderTree({ changedMap }: { changedMap: Map<string, DiffFile> }) {
  const paths = loadedOr(repoFiles.value, []);
  const tree = buildFolderTree(paths);

  return (
    <div class="folder-tree">
      <TreeNode
        node={tree}
        isExpanded={(path) => repoExpandedFolders.value.has(path)}
        onToggle={toggleRepoFolder}
        onFileClick={(path) => openRepoFilePreview(path, changedMap.has(path) ? 'diff' : 'file')}
        folderExtra={(folder) =>
          folderHasChanges(folder, changedMap) ? <span class="folder-change-dot" /> : null
        }
        fileExtra={(file) => {
          const info = changedMap.get(file.path);
          return info ? (
            <span class={`change-badge change-badge-${info.status}`}>
              {changeBadgeLabel(info.status)}
            </span>
          ) : null;
        }}
        fileClass={(file) => changedMap.has(file.path) ? 'repo-changed-file' : ''}
      />
    </div>
  );
}
