import type { DiffFile, DiffHunk } from '../../store/store';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { LineNumberedCode, type CodeRow } from './LineNumberedCode';

/** One side of one row of a side-by-side diff, or `null` where that side has no line
 *  (a filler, keeping the two columns aligned). */
export interface SideBySideCell {
  /** The line number in that side's file. */
  num: number;
  /** Index of the line within the hunk's `lines`, so the caller can look its
   *  highlighted HTML up. Kept as an index rather than the HTML itself so the
   *  alignment is pure and testable, and so the highlighter still runs over the
   *  whole hunk at once (a multi-line string or comment has to survive being
   *  split into rows). */
  index: number;
  kind: 'context' | 'change';
}

export interface SideBySideRow {
  left: SideBySideCell | null;
  right: SideBySideCell | null;
}

/** Pair a hunk's lines into rows for a side-by-side rendering.
 *
 *  A unified hunk is a sequence of context lines with runs of deletions and
 *  additions between them. Side by side, a run becomes rows that put the
 *  deletion and the addition that replaced it next to each other: deletion `i`
 *  beside addition `i`, and a filler on whichever side runs out first. A context
 *  line is the same text on both sides, so it emits one row with both.
 *
 *  Pairing by index within the run (rather than by content similarity) is what
 *  every side-by-side diff does: it costs nothing, and for the overwhelmingly common
 *  case of an edited line it puts the before and after on one row, which is the
 *  whole point of the view. A pure insertion is a run with no deletions, so its
 *  left side is filler all the way down, and a pure deletion the mirror. */
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
    // A context line closes the run before it: the deletions and additions
    // between two context lines are what replaced each other.
    flushRun();
    rows.push({
      left: { num: oldLine++, index, kind: 'context' },
      right: { num: newLine++, index, kind: 'context' },
    });
  });
  flushRun();

  return rows;
}

/** The two columns' rows for a whole file, plus the per-hunk separators.
 *
 *  Hunks are rendered as one continuous pair of columns with a separator row
 *  between them, rather than as separate column pairs per hunk: two `<pre>`s
 *  per hunk would each re-establish their own row heights, and the columns only
 *  stay aligned because every row is exactly one line tall in ONE flow. */
export interface SideBySideColumns {
  left: CodeRow[];
  right: CodeRow[];
}

/** Header text for the `@@` separator that begins a hunk, matching the unified
 *  view's so the two renderings read the same. */
export function hunkHeader(hunk: DiffHunk): string {
  return `@@ -${hunk.old_start},${hunk.old_count} +${hunk.new_start},${hunk.new_count} @@`;
}

/** Build both columns for a file. Each hunk contributes a separator row (a
 *  filler on both sides carrying the `@@` header) followed by its paired rows. */
export function sideBySideColumns(file: DiffFile, ext: string): SideBySideColumns {
  const isCode = CODE_EXTS.includes(ext);
  const left: CodeRow[] = [];
  const right: CodeRow[] = [];

  for (const hunk of file.hunks) {
    // Highlight the whole hunk in one pass, exactly as the unified view does:
    // `highlightFileLines` closes and reopens spans at each line break, so a
    // construct spanning several lines survives being split into rows.
    const hunkText = hunk.lines.map(l => l.content).join('\n');
    const highlighted = isCode
      ? highlightFileLines(hunkText, ext)
      : hunkText.split('\n').map(escapeHtml);

    // The same row object on both sides: rows are read-only render input, so
    // there is nothing for the two columns to disagree about.
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

/** One cell as a `CodeRow`: the line's highlighted HTML tinted by what happened
 *  to it, or an empty filler where this side has no line. */
function cellRow(cell: SideBySideCell | null, highlighted: string[], change: 'deletion' | 'addition'): CodeRow {
  if (cell === null) return { html: '', num: null, cls: 'side-by-side-diff-filler' };
  return {
    html: highlighted[cell.index] ?? '',
    num: cell.num,
    cls: cell.kind === 'change' ? `side-by-side-diff-${change}` : undefined,
  };
}

/** The side-by-side diff: the original on the left, the changed file on the
 *  right, aligned row for row.
 *
 *  Both columns are `LineNumberedCode`, so the line numbering, the row markup
 *  and the syntax highlighting are the file preview's own rather than a third
 *  implementation. They render with `selection="none"`: the left column's
 *  numbers are the OLD file's and the right column's the NEW file's, so one
 *  file-level `selectedLines` cannot mean both.
 *
 *  Alignment is structural rather than measured: `.line-content` is
 *  `white-space: pre`, so every row is exactly one line tall and the two
 *  columns stay in step by construction. That is also why this view cannot wrap
 *  long lines the way the unified one does; each column scrolls horizontally
 *  instead. */
export function SideBySideDiff({ file }: { file: DiffFile }) {
  const ext = file.path.split('.').pop()?.toLowerCase() || '';
  const { left, right } = sideBySideColumns(file, ext);

  return (
    <div class="side-by-side-diff" data-role="side-by-side-diff">
      <div class="side-by-side-diff-side" data-role="side-by-side-diff-original" aria-label="Original">
        <LineNumberedCode rows={left} selection="none" />
      </div>
      <div class="side-by-side-diff-side" data-role="side-by-side-diff-changed" aria-label="Changed">
        <LineNumberedCode rows={right} selection="none" />
      </div>
    </div>
  );
}
