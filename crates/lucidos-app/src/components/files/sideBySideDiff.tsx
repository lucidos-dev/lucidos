import type { DiffFile, DiffHunk } from '../../store/store';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { LineNumberedCode, type CodeRow } from './LineNumberedCode';

export interface SideBySideCell {
  num: number;
  index: number;
  kind: 'context' | 'change';
}

export interface SideBySideRow {
  left: SideBySideCell | null;
  right: SideBySideCell | null;
}

export function sideBySideRows(hunk: DiffHunk): SideBySideRow[] {
  const rows: SideBySideRow[] = [];
  let oldLine = hunk.old_start;
  let newLine = hunk.new_start;
  let deletions: SideBySideCell[] = [];
  let additions: SideBySideCell[] = [];

  const flushRun = () => {
    for (let i = 0; i < Math.max(deletions.length, additions.length); i++) {
      rows.push({ left: deletions[i] ?? null, right: additions[i] ?? null });
    }
    deletions = [];
    additions = [];
  };

  hunk.lines.forEach((line, index) => {
    if (line.type === 'deletion') {
      deletions.push({ num: oldLine++, index, kind: 'change' });
      return;
    }
    if (line.type === 'addition') {
      additions.push({ num: newLine++, index, kind: 'change' });
      return;
    }
    flushRun();
    rows.push({
      left: { num: oldLine++, index, kind: 'context' },
      right: { num: newLine++, index, kind: 'context' },
    });
  });
  flushRun();

  return rows;
}

export interface SideBySideColumns {
  left: CodeRow[];
  right: CodeRow[];
}

export function hunkHeader(hunk: DiffHunk): string {
  return `@@ -${hunk.old_start},${hunk.old_count} +${hunk.new_start},${hunk.new_count} @@`;
}

export function sideBySideColumns(file: DiffFile, ext: string): SideBySideColumns {
  const isCode = CODE_EXTS.includes(ext);
  const left: CodeRow[] = [];
  const right: CodeRow[] = [];

  for (const hunk of file.hunks) {
    const hunkText = hunk.lines.map(l => l.content).join('\n');
    const highlighted = isCode
      ? highlightFileLines(hunkText, ext)
      : hunkText.split('\n').map(escapeHtml);

    const header: CodeRow = { html: escapeHtml(hunkHeader(hunk)), num: null, cls: 'side-by-side-diff-hunk-header' };
    left.push(header);
    right.push(header);

    for (const row of sideBySideRows(hunk)) {
      left.push(cellRow(row.left, highlighted, 'deletion'));
      right.push(cellRow(row.right, highlighted, 'addition'));
    }
  }

  return { left, right };
}

function cellRow(cell: SideBySideCell | null, highlighted: string[], change: 'deletion' | 'addition'): CodeRow {
  if (cell === null) return { html: '', num: null, cls: 'side-by-side-diff-filler' };
  return {
    html: highlighted[cell.index] ?? '',
    num: cell.num,
    cls: cell.kind === 'change' ? `side-by-side-diff-${change}` : undefined,
  };
}

export function SideBySideDiff({ file }: { file: DiffFile }) {
  const ext = file.path.split('.').pop()?.toLowerCase() || '';
  const { left, right } = sideBySideColumns(file, ext);

  return (
    <div class="side-by-side-diff" data-role="side-by-side-diff">
      <div class="side-by-side-diff-side" data-role="side-by-side-diff-original" aria-label="Original">
        <LineNumberedCode rows={left} selection="none" wideLines="caller" />
      </div>
      <div class="side-by-side-diff-side" data-role="side-by-side-diff-changed" aria-label="Changed">
        <LineNumberedCode rows={right} selection="none" wideLines="caller" />
      </div>
    </div>
  );
}
