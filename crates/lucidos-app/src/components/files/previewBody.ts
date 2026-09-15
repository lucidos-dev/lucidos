import {
  RENDERABLE_EXTS,
  REPO_RENDERABLE_EXTS,
  IMAGE_EXTS,
  VIDEO_EXTS,
  AUDIO_EXTS,
  TEXT_EXTS,
  isEditableDataFile,
  previewMediaKind,
} from './previewExts';

/** The extension of `path`, lowercased, or `''` when it has none. */
export function previewExt(path: string): string {
  return path.split('.').pop()?.toLowerCase() || '';
}

/** Shown as an `<img>` by the data-file preview. The shared binary-image list
 *  plus SVG, which is text (XML) but renders as a picture until the Source
 *  toggle asks otherwise. */
export function isImageLike(ext: string): boolean {
  return IMAGE_EXTS.includes(ext) || ext === 'svg';
}

/** Which body the data-file preview renders (`FilePreviewInline`). */
export type DataPreviewBody =
  | 'editor' | 'image' | 'pdf' | 'video' | 'audio'
  | 'html' | 'markdown' | 'csv' | 'slides' | 'source' | 'unsupported';

/** Which body the repository-file preview renders (`RepoFileContent`). */
export type RepoPreviewBody =
  | 'image' | 'pdf' | 'video' | 'audio'
  | 'markdown' | 'csv' | 'svg' | 'source';

/** The body a workspace data file previews as.
 *
 *  Derived once here because the pane and its HEADER must agree on it. The
 *  header offers Wrap only over the line-numbered source view, and a control
 *  present over a picture lies about what the surface can do. Same move
 *  `diffBodyKind` makes for a diff, one file type along.
 *
 *  `sourceToggle` is the raw `filePreviewSource` signal, not the per-file
 *  answer. Only a richly rendered type has a source view to switch to. So the
 *  toggle means nothing for a `.rs` file, and must not turn an image into text.
 */
export function dataPreviewBody(
  path: string,
  opts: { sourceToggle: boolean; editing: boolean },
): DataPreviewBody {
  if (opts.editing && isEditableDataFile(path)) return 'editor';
  const ext = previewExt(path);
  const sourceView = opts.sourceToggle && RENDERABLE_EXTS.includes(ext);

  if (isImageLike(ext) && !(ext === 'svg' && sourceView)) return 'image';
  if (ext === 'pdf') return 'pdf';
  if (VIDEO_EXTS.includes(ext)) return 'video';
  if (AUDIO_EXTS.includes(ext)) return 'audio';
  // TEXT_EXTS is the dispatch gate, and SVG in source view is its sole
  // exception (see previewExts.ts). Anything else has no body to show.
  if (!TEXT_EXTS.includes(ext) && !(ext === 'svg' && sourceView)) return 'unsupported';

  if (sourceView) return 'source';
  if (ext === 'html' || ext === 'htm') return 'html';
  if (ext === 'md') return 'markdown';
  if (ext === 'csv') return 'csv';
  if (ext === 'slides') return 'slides';
  // Code, JSON, plain text, and any unknown-but-textual file.
  return 'source';
}

/** The body a repository file previews as, at whatever revision it is read.
 *
 *  Differs from the data-file answer in two ways, both deliberate. Repo HTML is
 *  source under review rather than a document, so it never renders live
 *  (`REPO_RENDERABLE_EXTS`). And an unknown or extensionless file is source
 *  here, where the data-file preview calls it unsupported.
 *
 *  `slides` is in `REPO_RENDERABLE_EXTS` but has no repo renderer, so it falls
 *  through to source. That is the existing behaviour, kept rather than fixed
 *  here: changing it is a preview feature, not a wrapping fix. */
export function repoPreviewBody(
  path: string,
  opts: { sourceToggle: boolean },
): RepoPreviewBody {
  const ext = previewExt(path);
  const media = previewMediaKind(ext);
  if (media !== 'text') return media;

  const rendered = !opts.sourceToggle && REPO_RENDERABLE_EXTS.includes(ext);
  if (rendered) {
    if (ext === 'md') return 'markdown';
    if (ext === 'csv') return 'csv';
    if (ext === 'svg') return 'svg';
  }
  return 'source';
}
