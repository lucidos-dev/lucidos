import { useEffect } from 'preact/hooks';
import { artifacts, repositories, repoSource, repoPending, workspaceName } from '../../store/store';
import { uploadFiles } from '../../store/actions/artifacts';
import { loadRepositories } from '../../store/actions/chat';
import { switchRepoSource } from '../../store/actions/repositories';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { FolderTree } from './FolderTree';
import { RepoFilesView } from './RepoFilesView';
import { ChangeSelector } from './ChangeSelector';
import { Dropdown } from '../shared/Dropdown';
import { HiddenFileInput } from '../shared/HiddenFileInput';
import { ListSkeleton } from '../shared/ListSkeleton';
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
    { value: '', label: `Current Workspace (${workspaceName.value || 'unknown'})` },
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
      <div class="content-view active">
        {hasSwitcher && (
          <div class="files-source-switcher">
            {/* Source switcher renders even in app-CC mode — selecting
                "Current Workspace" routes back through the dropdown's
                onChange, which clears repoPending and drops the user back
                into WorkspaceFilesView. Without this, an app-CC diff has no
                in-pane escape control. */}
            {sourceDropdown}
            {isRepo && <ChangeSelector />}
          </div>
        )}
        <RepoFilesView />
      </div>
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
    <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeleton fill />}>
      {loadable.status === 'loaded' ? (
        <div class="artifacts-desktop">
          {loadable.data.length > 0 && <FolderTree />}
        </div>
      ) : null}
    </LoadingFade>
  );
}
