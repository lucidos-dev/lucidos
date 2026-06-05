// Extensions FilePreviewInline / RepoFilePreview render via the rich preview
// path (markdown, slides, csv, html, svg) instead of raw text/diff.
export const RENDERABLE_EXTS = ['md', 'html', 'htm', 'csv', 'svg', 'slides'];

// Extensions FilePreviewInline treats as text — shown as syntax-highlighted
// source (or the rendered view for the RENDERABLE_EXTS subset) and editable
// inline. Excludes binary previews (image / pdf / audio / video).
export const TEXT_EXTS = [
  'txt', 'md', 'json', 'csv', 'js', 'ts', 'jsx', 'tsx', 'css', 'html', 'xml',
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
