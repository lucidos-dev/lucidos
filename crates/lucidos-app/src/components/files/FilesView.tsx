import type { VNode } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { artifacts, repositories, repoSource, repoPending, repoFiles, repoDiff, repoViewMode, visibleWorkspaceName } from '../../store/store';
import { uploadFiles } from '../../store/actions/artifacts';
import { loadRepositories } from '../../store/actions/chat';
import { switchRepoSource } from '../../store/actions/repositories';
import { useDelayedLoading, useDelayedFlag } from '../../hooks/useDelayedLoading';
import { FolderTree, folderTreeSkeletonRow } from './FolderTree';
import { RepoFilesView, repoFilesContentReady } from './RepoFilesView';
import { ChangeSelector } from './ChangeSelector';
import { Dropdown } from '../shared/Dropdown';
import { HiddenFileInput } from '../shared/HiddenFileInput';
import { ListSkeletonOf, SkeletonProvider, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { loadedOr } from '../../store/types';

export function FilesView() {
  useEffect(() => {
    if (repositories.value.status === 'not-loaded') void loadRepositories();
  }, []);
  const repos = loadedOr(repositories.value, []);

  const isRepo = repoSource.value !== null;
  // App coding-agent threads have no registered repo (the workspace itself
  // is the git root). viewThreadCcDiff sets repoPending + repoDiff in that
  // case; RepoFilesView's no-registered-repo branch knows how to render the
  // diff inline. Without this fallback the user lands on WorkspaceFilesView
  // (the artifacts tree) and the diff they clicked is invisible.
  const isAppCcDiff = !isRepo && repoPending.value != null;
  const showRepoView = isRepo || isAppCcDiff;

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

  // Workspace (artifacts) mode. The whole area stays an import drop target, and
  // the import control is pinned to the top-right of the switcher row — the
  // source-switcher selector (when present) owns the top-left, the import box
  // the top-right. The switcher row renders even without registered repos so
  // the import box always has a top row to anchor to.
  return (
    <div class="content-view active">
      <div class="workspace-files-view" data-drop-zone="import">
        <div class="files-source-switcher">
          {sourceDropdown}
          <ImportDropzone />
        </div>
        <WorkspaceFilesView />
      </div>
    </div>
  );
}

/** Repo / app-CC diff view: the source switcher + change selector on top and the
 *  repo files (or diff list) below. They fade IN TOGETHER once the content is
 *  ready — the switcher no longer pops in ahead of the file list (the staggered
 *  load). A latch keeps the switcher mounted through later All Files ↔ Changes
 *  reloads (RepoFilesView owns those with its own fade); only the FIRST paint
 *  waits, so subsequent mode toggles never yank the switcher back out. */
function RepoContentView({
  hasSwitcher,
  sourceDropdown,
  showChangeSelector,
}: {
  hasSwitcher: boolean;
  sourceDropdown: VNode | null;
  showChangeSelector: boolean;
}) {
  // Mirrors RepoFilesView's own readiness (changes → diff, all → tree), so the
  // whole unit fades in exactly when RepoFilesView would first show content.
  const contentReady = repoFilesContentReady({
    filesStatus: repoFiles.value.status,
    diffStatus: repoDiff.value.status,
    mode: repoViewMode.value,
  });
  // A failed diff/tree load never becomes "ready", so also reveal the real view
  // on failure — otherwise RepoFilesView (which owns the actionable "Failed to
  // load" branches) would never mount and the user would sit on the skeleton.
  const loadFailed = repoDiff.value.status === 'failed' || repoFiles.value.status === 'failed';
  const settled = contentReady || loadFailed;
  // Latch on first-settled: a ref (not state) — the flip already re-renders, and
  // this must never go back to false or a mid-session reload (which drops
  // contentReady) would yank the switcher out from under the user.
  const everShown = useRef(false);
  if (settled) everShown.current = true;
  const showSkeleton = useDelayedFlag(!everShown.current);

  return (
    <div class="content-view active">
      <LoadingFade showSkeleton={showSkeleton} skeleton={<RepoViewSkeleton hasSwitcher={hasSwitcher} />}>
        {everShown.current ? (
          <>
            {hasSwitcher && (
              <div class="files-source-switcher">
                {/* Source switcher renders even in app-CC mode — selecting
                    "Current Workspace" routes back through the dropdown's
                    onChange, which clears repoPending and drops the user back
                    into WorkspaceFilesView. Without this, an app-CC diff has no
                    in-pane escape control. */}
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

/** Placeholder for RepoContentView: a switcher-shaped row (when a switcher will
 *  show) above the file-list skeleton, so the crossfade lands on the same
 *  layout the loaded content occupies. `fill` sizes the list run to the pane;
 *  the switcher row above it correctly shrinks the measured height. */
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

  // Crossfade the skeleton out as the artifacts area appears. The import
  // dropzone + source switcher now live in the parent's `files-source-switcher`
  // row (main refactor), so this only owns the artifacts list.
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
