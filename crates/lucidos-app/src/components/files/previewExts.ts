// Extensions FilePreviewInline / RepoFilePreview render via the rich preview
// path (markdown, slides, csv, html, svg) instead of raw text/diff.
export const RENDERABLE_EXTS = ['md', 'html', 'htm', 'csv', 'svg', 'slides'];

// Extensions the REPO file/diff preview renders richly. HTML is deliberately
// excluded: a repo HTML file is application source under review, not a
// self-contained document, so a live `srcDoc` render shows misleading
// pre-hydration markup — e.g. the app shell's inlined boot splash ("Opening
// your workspace…") for crates/lucidos-app/index.html — and its relative
// asset/script refs can't resolve in srcDoc anyway. It's shown as
// syntax-highlighted source instead. Markdown/CSV/SVG/slides are self-contained
// and stay rendered. (The artifact viewer keeps HTML via RENDERABLE_EXTS —
// artifacts are authored, standalone documents.)
export const REPO_RENDERABLE_EXTS = RENDERABLE_EXTS.filter(e => e !== 'html' && e !== 'htm');

// Binary-media extensions both file viewers render via a URL-pointed element
// (<img>/<video>/<audio>/<iframe>), never by fetching the bytes as text. SVG is
// deliberately NOT here — it's XML, so it gets the rich/source text path
// (RENDERABLE_EXTS) and only FilePreviewInline shows it as an <img> by default.
export const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'ico', 'bmp'];
// .ogg is treated as audio (Vorbis/Opus is by far the most common modern usage);
// .ogv is the video variant. Listing 'ogg' in both video and audio caused
// double-render of <video> and <audio> for the same file.
export const VIDEO_EXTS = ['mp4', 'webm', 'ogv', 'mov'];
export const AUDIO_EXTS = ['mp3', 'wav', 'ogg', 'flac', 'm4a'];

/** How a file extension should be previewed. `'text'` is the default
 *  (syntax-highlighted source / rich render) — it covers SVG and every
 *  extensionless or unknown-but-textual repo file (Makefile, LICENSE, …), so
 *  callers must never gate the text path on a known-extension allow-list. The
 *  binary kinds are diverted to a URL-pointed media element instead. */
export type PreviewMediaKind = 'image' | 'video' | 'audio' | 'pdf' | 'text';
export function previewMediaKind(ext: string): PreviewMediaKind {
  if (IMAGE_EXTS.includes(ext)) return 'image';
  if (VIDEO_EXTS.includes(ext)) return 'video';
  if (AUDIO_EXTS.includes(ext)) return 'audio';
  if (ext === 'pdf') return 'pdf';
  return 'text';
}

// Extensions FilePreviewInline treats as text — shown as syntax-highlighted
// source (or the rendered view for the RENDERABLE_EXTS subset) and editable
// inline. Excludes binary previews (image / pdf / audio / video).
export const TEXT_EXTS = [
  'txt', 'md', 'json', 'csv', 'js', 'gs', 'ts', 'jsx', 'tsx', 'css', 'html', 'xml',
  'py', 'rb', 'go', 'rs', 'java', 'kt', 'kts', 'c', 'cpp', 'h', 'sh', 'bash', 'zsh',
  'yaml', 'yml', 'toml', 'ini', 'cfg', 'conf', 'log', 'sql', 'graphql',
  'vue', 'svelte', 'slides',
];

/** True when the data-file preview can edit `path` inline: a text file (or an
 *  SVG, which is text but previewed as an image by default), and not under the
 *  engine-shipped read-only `system-knowhow/` tree — the PUT /api/v1/data
 *  endpoint rejects writes there. Repo-file paths (`repo:…`, read at a git ref)
 *  are filtered out by the caller, not here. */
export function isEditableDataFile(path: string): boolean {
  const ext = path.split('.').pop()?.toLowerCase() || '';
  const editableExt = TEXT_EXTS.includes(ext) || ext === 'svg';
  return editableExt && !path.startsWith('system-knowhow/');
}
