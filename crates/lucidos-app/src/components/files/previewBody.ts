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

export function previewExt(path: string): string {
  return path.split('.').pop()?.toLowerCase() || '';
}

export function isImageLike(ext: string): boolean {
  return IMAGE_EXTS.includes(ext) || ext === 'svg';
}

export type DataPreviewBody =
  | 'editor' | 'image' | 'pdf' | 'video' | 'audio'
  | 'html' | 'markdown' | 'csv' | 'slides' | 'source' | 'unsupported';

export type RepoPreviewBody =
  | 'image' | 'pdf' | 'video' | 'audio'
  | 'markdown' | 'csv' | 'source';

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
  if (!TEXT_EXTS.includes(ext) && !(ext === 'svg' && sourceView)) return 'unsupported';

  if (sourceView) return 'source';
  if (ext === 'html' || ext === 'htm') return 'html';
  if (ext === 'md') return 'markdown';
  if (ext === 'csv') return 'csv';
  if (ext === 'slides') return 'slides';
  return 'source';
}

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
    if (ext === 'svg') return 'image';
  }
  return 'source';
}
