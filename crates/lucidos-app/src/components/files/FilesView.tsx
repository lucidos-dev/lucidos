import { useEffect } from 'preact/hooks';
import { artifacts, repositories, repoSource, repoPending, workspaceName } from '../../store/store';
import { expandAllFolders, collapseAllFolders, uploadFiles } from '../../store/actions/artifacts';
import { loadRepositories } from '../../store/actions/chat';
import { switchRepoSource } from '../../store/actions/repositories';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { FolderTree } from './FolderTree';
import { RepoFilesView } from './RepoFilesView';
import { ChangeSelector } from './ChangeSelector';
import { Dropdown } from '../shared/Dropdown';
import { HiddenFileInput } from '../shared/HiddenFileInput';
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

  return (
    <div class="content-view active">
      {sourceOptions.length > 0 && (
        <div class="files-source-switcher">
          {/* Source switcher renders even in app-CC mode — selecting
              "Current Workspace" routes back through the dropdown's
              onChange, which clears repoPending and drops the user back
              into WorkspaceFilesView. Without this, an app-CC diff has no
              in-pane escape control. */}
          <Dropdown
            options={sourceOptions}
            value={repoSource.value ?? ''}
            onChange={(v) => void switchRepoSource(v || null)}
          />
          {isRepo && <ChangeSelector />}
        </div>
      )}
      {showRepoView ? <RepoFilesView /> : <WorkspaceFilesView />}
    </div>
  );
}

function WorkspaceFilesView() {
  const loadable = artifacts.value;
  const showLoading = useDelayedLoading(loadable);

  const handleFileSelected = (e: Event) => {
    const input = e.target as HTMLInputElement;
    if (input.files && input.files.length > 0) {
      void uploadFiles(input.files);
      input.value = '';
    }
  };

  if (loadable.status === 'failed') {
    return (
      <div class="files-toolbar">
        <span class="files-hint error-text">Failed to load files: {loadable.error}</span>
      </div>
    );
  }

  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return <div class="loading-spinner" />;
  }

  const hasArtifacts = loadable.data.length > 0;

  return (
    <div class="workspace-files-view" data-drop-zone="import">
      <div class="files-toolbar">
        <span class="files-toolbar-actions">
          {hasArtifacts && (
            <>
              <button class="files-toolbar-btn" onClick={expandAllFolders} data-tooltip="Expand all folders">Expand All</button>
              <button class="files-toolbar-btn" onClick={collapseAllFolders} data-tooltip="Collapse all folders">Collapse All</button>
            </>
          )}
          <label class="files-toolbar-btn">
            <HiddenFileInput multiple onChange={handleFileSelected} />
            Import
          </label>
        </span>
        <span class="files-hint">Drop here to import</span>
      </div>
      <div class="artifacts-desktop">
        {hasArtifacts && <FolderTree />}
      </div>
    </div>
  );
}
