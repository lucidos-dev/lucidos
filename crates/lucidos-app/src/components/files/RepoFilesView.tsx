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
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { FileTypeIcon } from '../../utils/fileIcons';
import { TreeNode, folderTreeSkeletonRow, useRevealedFolderScroll } from './FolderTree';
import { changeBadgeLabel } from './changeBadge';
import { diffStats } from './diffStats';
import { DiffSkeleton, DiffStatsInline, DiffView } from './DiffView';

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
  const hasRepo = repoSource.value != null;

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

  if (!hasRepo) {
    return (
      <LoadingFade showSkeleton={diffShowLoading} skeleton={<DiffSkeleton files={2} class="folder-tree" />}>
        {diffLoaded && (
          <div class="artifacts-desktop">
            <InlineDiffList files={changedFiles} />
          </div>
        )}
      </LoadingFade>
    );
  }

  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf fill containerClass="folder-tree" row={folderTreeSkeletonRow} />}>
      {contentReady ? (
        <>
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
        <ChangedFileRow key={f.path} file={f} stats={fileStats[i]} active={f.path === activePath} />
      ))}
      {files.length === 0 && (
        <div class="empty-state">No changes</div>
      )}
    </div>
  );
}

function ChangedFileRow({ file, stats, active = false }: {
  file?: DiffFile;
  stats?: { additions: number; deletions: number };
  active?: boolean;
}) {
  return (
    <div
      class={`file-item repo-changed-file${active ? ' active' : ''}`}
      onClick={file ? () => openRepoFilePreview(file.path, 'diff') : undefined}
    >
      <SkBlock w="1.25rem" h="1.25rem" round>
        <FileTypeIcon path={file?.path ?? ''} className="file-icon" />
      </SkBlock>
      <SkText class="file-name file-path" w="11rem">{file?.path}</SkText>
      {stats && <DiffStatsInline additions={stats.additions} deletions={stats.deletions} />}
      <SkBlock w="1.25rem" h="1rem" round>
        {file && (
          <span class={`change-badge change-badge-${file.status}`}>
            {changeBadgeLabel(file.status)}
          </span>
        )}
      </SkBlock>
    </div>
  );
}

export function ChangesFileListSkeleton() {
  return <ListSkeletonOf count={6} containerClass="folder-tree" row={() => <ChangedFileRow />} />;
}

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
  const treeRef = useRevealedFolderScroll();

  return (
    <div class="folder-tree" ref={treeRef}>
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
      />
    </div>
  );
}
