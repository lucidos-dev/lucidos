import { splitPreviewPath } from '../../utils/previewPath';

/** The previewed file's whole path, on its own row above the preview.
 *
 *  The header bar names the FILE, and the bar is the narrowest surface a title
 *  ever appears on: the mobile content row holds its title inside a fixed-width
 *  cluster whose span is what pins the nav chevrons to the same two screen
 *  positions on every pane (see `.header-nav-cluster` in styles/header-mark.css),
 *  which on a phone leaves about a dozen characters. So `system-knowhow.md`
 *  arrives up there as `system-knowh…`, and a nested path could never arrive at
 *  all. This row is the pane's full width and WRAPS rather than truncating, so
 *  the whole path is readable however deep it is.
 *
 *  Two spans rather than one string, because a path read at a glance is mostly
 *  folders: the name is the emphasized half, so the eye still lands on the file
 *  the way it does on the title above. */
export function FilePreviewPath({ path }: { path: string }) {
  const { dir, name } = splitPreviewPath(path);

  return (
    <div class="file-preview-path">
      {dir && <span class="file-preview-path-dir">{dir}</span>}
      <span class="file-preview-path-name">{name}</span>
    </div>
  );
}
