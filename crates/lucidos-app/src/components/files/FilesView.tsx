import type { VNode } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { artifacts, repositories, repoSource, repoFiles, repoDiff, repoViewMode, revealedFolder, visibleWorkspaceName } from '../../store/store';
import { loadArtifacts, uploadFiles } from '../../store/actions/artifacts';
import { loadRepositories } from '../../store/actions/chat';
import { refreshRepositories } from '../../store/actions/repositoriesLoader';
import { refreshRepoView, switchRepoSource } from '../../store/actions/repositories';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useDelayedLoading, useDelayedFlag } from '../../hooks/useDelayedLoading';
import { FolderTree, folderTreeSkeletonRow } from './FolderTree';
import { RepoFilesView, repoFilesContentReady } from './RepoFilesView';
import { ChangeSelector } from './ChangeSelector';
import { Dropdown } from '../shared/Dropdown';
import { HiddenFileInput } from '../shared/HiddenFileInput';
import { ListSkeletonOf, SkeletonProvider, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { loadedOr } from '../../store/types';

function refreshFilesPanel(): Promise<unknown> {
  const repoId = repoSource.peek();
  return Promise.all([
    repoId ? refreshRepoView(repoId) : loadArtifacts(),
    refreshRepositories(),
  ]);
}

export function FilesView() {
  useEffect(() => {
    if (repositories.value.status === 'not-loaded') void loadRepositories();
  }, []);
  useEffect(() => () => { revealedFolder.value = null; }, []);
  usePanelRefresh('files', refreshFilesPanel);
  const repos = loadedOr(repositories.value, []);

  const isRepo = repoSource.value !== null;
  const showRepoView = isRepo
    || (repoViewMode.value === 'changes' && repoDiff.value.status !== 'failed');

  const sourceOptions = repos.length > 0 ? [
    { value: '', label: `Current Workspace (${visibleWorkspaceName.value || 'unknown'})` },
    ...repos.map(r => ({ value: r.id, label: r.name })),
  ] : [];
  const hasSwitcher = sourceOptions.length > 0;

  const sourceDropdown = hasSwitcher ? (
    <Dropdown
      options={sourceOptions}
      value={repoSource.value ?? ''}
      onChange={(v) => void switchRepoSource(v || null)}
    />
  ) : null;

  if (showRepoView) {
    return (
      <RepoContentView
        hasSwitcher={hasSwitcher}
        sourceDropdown={sourceDropdown}
        showChangeSelector={isRepo}
      />
    );
  }

  return (
    <div class="content-view active content-view-full-bleed" data-drop-zone="import">
      <div class="workspace-files-view">
        <div class="files-source-switcher">
          {sourceDropdown}
          <ImportDropzone />
        </div>
        <WorkspaceFilesView />
      </div>
    </div>
  );
}

function RepoContentView({
  hasSwitcher,
  sourceDropdown,
  showChangeSelector,
}: {
  hasSwitcher: boolean;
  sourceDropdown: VNode | null;
  showChangeSelector: boolean;
}) {
  const contentReady = repoFilesContentReady({
    filesStatus: repoFiles.value.status,
    diffStatus: repoDiff.value.status,
    mode: repoViewMode.value,
  });
  const loadFailed = repoDiff.value.status === 'failed' || repoFiles.value.status === 'failed';
  const settled = contentReady || loadFailed;
  const everShown = useRef(false);
  if (settled) everShown.current = true;
  const showSkeleton = useDelayedFlag(!everShown.current);

  return (
    <div class="content-view active content-view-full-bleed">
      <LoadingFade showSkeleton={showSkeleton} skeleton={<RepoViewSkeleton hasSwitcher={hasSwitcher} />}>
        {everShown.current ? (
          <>
            {hasSwitcher && (
              <div class="files-source-switcher">
                {sourceDropdown}
                {showChangeSelector && <ChangeSelector />}
              </div>
            )}
            <RepoFilesView />
          </>
        ) : null}
      </LoadingFade>
    </div>
  );
}

function RepoViewSkeleton({ hasSwitcher }: { hasSwitcher: boolean }) {
  return (
    <SkeletonProvider>
      {hasSwitcher && (
        <div class="files-source-switcher" aria-hidden="true">
          <SkBlock w="12rem" h="1.75rem" round />
          <SkBlock w="10rem" h="1.75rem" round />
        </div>
      )}
      <ListSkeletonOf fill containerClass="folder-tree" row={folderTreeSkeletonRow} />
    </SkeletonProvider>
  );
}

function ImportDropzone() {
  const handleFileSelected = (e: Event) => {
    const input = e.target as HTMLInputElement;
    if (input.files && input.files.length > 0) {
      void uploadFiles(input.files);
      input.value = '';
    }
  };

  return (
    <label class="files-import-dropzone">
      <HiddenFileInput multiple onChange={handleFileSelected} />
      Drop or click to import
    </label>
  );
}

function WorkspaceFilesView() {
  const loadable = artifacts.value;
  const showLoading = useDelayedLoading(loadable);

  if (loadable.status === 'failed') {
    return (
      <div class="files-toolbar">
        <span class="files-hint error-text">Failed to load files: {loadable.error}</span>
      </div>
    );
  }

  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf fill containerClass="folder-tree" row={folderTreeSkeletonRow} />}>
      {loadable.status === 'loaded' ? (
        <div class="artifacts-desktop">
          {loadable.data.length > 0 && <FolderTree />}
        </div>
      ) : null}
    </LoadingFade>
  );
}
