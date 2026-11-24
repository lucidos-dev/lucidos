import { splitPreviewPath } from '../../utils/previewPath';
import { revealFolderInFiles } from '../../store/actions/repositories';

export function FilePreviewPath({ path }: { path: string }) {
  const { dir, name } = splitPreviewPath(path);
  const folders = dir.split('/').filter(Boolean);

  return (
    <nav class="file-preview-path" aria-label="File path">
      {folders.map((folder, i) => {
        const folderPath = folders.slice(0, i + 1).join('/');
        return (
          <span key={folderPath} class="file-preview-path-crumb">
            <button
              type="button"
              class="file-preview-path-folder"
              data-tooltip="Show in Files"
              onClick={() => revealFolderInFiles(path, folderPath)}
            >
              {folder}
            </button>
            <span class="file-preview-path-sep" aria-hidden="true">›</span>
          </span>
        );
      })}
      <span class="file-preview-path-name">{name}</span>
    </nav>
  );
}
