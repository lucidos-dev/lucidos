export const RENDERABLE_EXTS = ['md', 'html', 'htm', 'csv', 'svg', 'slides'];

export const REPO_RENDERABLE_EXTS = RENDERABLE_EXTS.filter(e => e !== 'html' && e !== 'htm');

export const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'ico', 'bmp'];
export const VIDEO_EXTS = ['mp4', 'webm', 'ogv', 'mov'];
export const AUDIO_EXTS = ['mp3', 'wav', 'ogg', 'flac', 'm4a'];

export type PreviewMediaKind = 'image' | 'video' | 'audio' | 'pdf' | 'text';
export function previewMediaKind(ext: string): PreviewMediaKind {
  if (IMAGE_EXTS.includes(ext)) return 'image';
  if (VIDEO_EXTS.includes(ext)) return 'video';
  if (AUDIO_EXTS.includes(ext)) return 'audio';
  if (ext === 'pdf') return 'pdf';
  return 'text';
}

export const TEXT_EXTS = [
  'txt', 'md', 'json', 'csv', 'js', 'gs', 'ts', 'jsx', 'tsx', 'css', 'html', 'htm', 'xml',
  'py', 'rb', 'go', 'rs', 'java', 'kt', 'kts', 'c', 'cpp', 'h', 'sh', 'bash', 'zsh',
  'yaml', 'yml', 'toml', 'ini', 'cfg', 'conf', 'log', 'sql', 'graphql',
  'vue', 'svelte', 'slides',
];

export function isEditableDataFile(path: string): boolean {
  const ext = path.split('.').pop()?.toLowerCase() || '';
  const editableExt = TEXT_EXTS.includes(ext) || ext === 'svg';
  return editableExt && !path.startsWith('system-knowhow/');
}
