// A deliberately small, shared icon vocabulary: every file reads as a clean
// page-with-folded-corner, distinguished only by one of three calm inner marks
// (document lines, code chevrons, image horizon). We avoid per-extension
// pictograms — at row size they turn busy and heavy; a light outline with a
// single restrained mark stays legible without the clutter.
type FileIconKind = 'document' | 'code' | 'image';

const CODE_EXTENSIONS = new Set([
  'c', 'cc', 'cpp', 'cs', 'css', 'go', 'h', 'hpp', 'html', 'java', 'js',
  'jsx', 'kt', 'php', 'py', 'rb', 'rs', 'scss', 'sh', 'swift', 'ts', 'tsx',
]);
const IMAGE_EXTENSIONS = new Set(['gif', 'jpeg', 'jpg', 'png', 'svg', 'webp']);

function fileIconKind(path: string): FileIconKind {
  const name = path.toLowerCase().split('/').pop() ?? '';
  const ext = name.includes('.') ? name.split('.').pop() ?? '' : '';

  if (CODE_EXTENSIONS.has(ext)) return 'code';
  if (IMAGE_EXTENSIONS.has(ext)) return 'image';
  return 'document';
}

function fileMark(kind: FileIconKind) {
  switch (kind) {
    case 'code':
      return (
        <>
          <path d="M8.25 10 6.75 11.75l1.5 1.75" />
          <path d="M11.75 10 13.25 11.75l-1.5 1.75" />
        </>
      );
    case 'image':
      return (
        <>
          <circle cx="8" cy="10" r="0.85" fill="currentColor" stroke="none" />
          <path d="M6.75 13.25 8.75 11l1.75 1.75 1.5-1.5 1.75 2" />
        </>
      );
    case 'document':
      return (
        <>
          <path d="M7.25 10.75h5.5" />
          <path d="M7.25 13.25h3.75" />
        </>
      );
  }
}

export function FileTypeIcon({ path, className }: { path: string; className?: string }) {
  return (
    <svg class={className} viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M6.25 3.5H12.5L15.5 6.5V14.75A1.75 1.75 0 0 1 13.75 16.5H6.25A1.75 1.75 0 0 1 4.5 14.75V5.25A1.75 1.75 0 0 1 6.25 3.5Z" />
      <path d="M12.5 3.5V6.5H15.5" />
      {fileMark(fileIconKind(path))}
    </svg>
  );
}

export function FolderIcon({ className }: { className?: string }) {
  return (
    <svg class={className} viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M3 6.75a1.75 1.75 0 0 1 1.75-1.75H8l1.6 1.75h5.65A1.75 1.75 0 0 1 17 8.5v5.25A1.75 1.75 0 0 1 15.25 15.5H4.75A1.75 1.75 0 0 1 3 13.75z" />
    </svg>
  );
}
