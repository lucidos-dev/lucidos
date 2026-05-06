import { artifacts, repositories, repoSource, workspaceName } from '../../store/store';
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
  if (repositories.value.status === 'not-loaded') loadRepositories();
  const repos = loadedOr(repositories.value, []);

  const isRepo = repoSource.value !== null;

  const sourceOptions = repos.length > 0 ? [
    { value: '', label: `Current Workspace (${workspaceName.value || 'unknown'})` },
    ...repos.map(r => ({ value: r.id, label: r.name })),
  ] : [];

  return (
    <div class="content-view active">
      {sourceOptions.length > 0 && (
        <div class="files-source-switcher">
          <Dropdown
            options={sourceOptions}
            value={repoSource.value ?? ''}
            onChange={(v) => switchRepoSource(v || null)}
          />
          {isRepo && <ChangeSelector />}
        </div>
      )}
      {isRepo ? <RepoFilesView /> : <WorkspaceFilesView />}
    </div>
  );
}

function WorkspaceFilesView() {
  const loadable = artifacts.value;
  const showLoading = useDelayedLoading(loadable);

  const handleFileSelected = (e: Event) => {
    const input = e.target as HTMLInputElement;
    if (input.files && input.files.length > 0) {
      uploadFiles(input.files);
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
