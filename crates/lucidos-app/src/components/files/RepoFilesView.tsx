import {
  repoFiles, repoDiff, repoViewMode, repoExpandedFolders,
  repoPending, repoSelectedChangeId, repoSource,
} from '../../store/store';
import type { DiffFile } from '../../store/store';
import {
  toggleRepoFolder, expandAllRepoFolders, collapseAllRepoFolders,
  openRepoFilePreview,
} from '../../store/actions/repositories';
import { buildFolderTree } from '../../store/actions/artifacts';
import type { FolderNode } from '../../store/actions/artifacts';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { loadedOr } from '../../store/types';
import { getEmojiForFile } from '../../utils/fileIcons';
import { TreeNode } from './FolderTree';
import { changeBadgeLabel } from './changeBadge';
import { diffStats } from './diffStats';
import { DiffStatsInline, DiffView } from './DiffView';

export function RepoFilesView() {
  const loadable = repoFiles.value;
  const showLoading = useDelayedLoading(loadable);
  const diffLoadable = repoDiff.value;
  // App-CC branch shows the diff directly (no repoFiles tree), so its spinner
  // must key off the diff's own load state, not repoFiles'.
  const diffShowLoading = useDelayedLoading(diffLoadable);
  const pending = repoPending.value;
  const mode = repoViewMode.value;
  // Without a registered repo (app coding-agent threads use the workspace
  // as their git root, which isn't a Repository row), the "All Files" tab
  // would render an empty tree — hide the toggle and pin Changes view.
  // The repo-file tree (repoFiles) isn't fetched for the app-CC path either,
  // so don't gate the render on its load state.
  const hasRepo = repoSource.value != null;

  if (hasRepo) {
    if (loadable.status === 'failed') {
      return (
        <div class="files-toolbar">
          <span class="files-hint error-text">Failed to load: {loadable.error}</span>
        </div>
      );
    }
    if (loadable.status !== 'loaded') {
      if (!showLoading) return null;
      return <div class="loading-spinner" />;
    }
  }

  if (diffLoadable.status === 'failed') {
    return (
      <div class="files-toolbar">
        <span class="files-hint error-text">Failed to load diff: {diffLoadable.error}</span>
      </div>
    );
  }

  const diffLoaded = diffLoadable.status === 'loaded';
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
    <>
      <div class="files-toolbar files-toolbar-bordered">
        <span class="files-toolbar-actions">
          {(pending || repoSelectedChangeId.value) && (
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
          )}
          {mode === 'all' && (
            <>
              <button class="files-toolbar-btn" onClick={expandAllRepoFolders} data-tooltip="Expand all folders">Expand All</button>
              <button class="files-toolbar-btn" onClick={collapseAllRepoFolders} data-tooltip="Collapse all folders">Collapse All</button>
            </>
          )}
        </span>
      </div>
      <div class="artifacts-desktop">
        {mode === 'changes' ? (
          <ChangesFileList files={changedFiles} />
        ) : (
          <RepoFolderTree changedMap={changedMap} />
        )}
      </div>
    </>
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
          <span class="file-icon">{getEmojiForFile(f.path)}</span>
          <span class="file-name">{f.path}</span>
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
function InlineDiffList({ files }: { files: DiffFile[] }) {
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
        indent={0}
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
