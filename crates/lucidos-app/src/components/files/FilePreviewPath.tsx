import { splitPreviewPath } from '../../utils/previewPath';
import { revealFolderInFiles } from '../../store/actions/repositories';

/** The previewed file's whole path, as a breadcrumb on its own row above the
 *  preview.
 *
 *  The header bar names the FILE, and the bar is the narrowest surface a title
 *  ever appears on: the mobile content row holds its title inside a fixed-width
 *  cluster whose span is what pins the nav chevrons to the same two screen
 *  positions on every pane (see `.header-nav-cluster` in styles/header-mark.css),
 *  which on a phone leaves about a dozen characters. This row is the pane's full
 *  width and WRAPS rather than truncating, so the whole path is readable however
 *  deep it is.
 *
 *  Each folder is a button that opens the Files view on it. The name is the
 *  emphasized end, so the eye lands on the file the way it does on the title
 *  above. */
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
