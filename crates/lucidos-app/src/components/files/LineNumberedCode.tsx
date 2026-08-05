import { useCallback, useRef } from 'preact/hooks';
import { useSignalEffect } from '@preact/signals';
import { selectedLines, consumeLineScrollTarget } from '../../store/store';

/** One rendered row of code. */
export interface CodeRow {
  /** The row's content, already syntax-highlighted or HTML-escaped by the
   *  caller. Injected as HTML, so a caller rendering untrusted text must escape
   *  it first (`escapeHtml`) rather than passing it through raw. */
  html: string;
  /** The number shown in the gutter, or `null` for a FILLER row: one that
   *  exists only to keep a side-by-side diff's two columns lined up where one side has
   *  no line at all. A filler row shows no number and is never selectable. */
  num: number | null;
  /** Extra class on the row, for a caller that tints rows (the side-by-side diff's
   *  addition / deletion shading). */
  cls?: string;
}

/** How these rows relate to the previewed file's own line numbering.
 *
 *  `file` means they ARE the file's lines: clicking a number selects into the
 *  `selectedLines` store signal, that selection renders as a highlight here, and
 *  a pending `lineScrollTarget` is consumed and scrolled to here.
 *
 *  `none` means they are a derived view: one column of a side-by-side diff, whose
 *  numbers are the OLD file's on the left and the NEW file's on the right. A
 *  file-level selection cannot mean both, so all three behaviours are off.
 *  Gating the scroll consumption matters as much as gating the click: a column
 *  that consumed a pending target would swallow a navigate meant for a file
 *  view, and (finding no such row) null the selection out from under it. */
export type LineSelectionMode = 'file' | 'none';

interface Props {
  rows: CodeRow[];
  selection?: LineSelectionMode;
}

/** Turn a file's lines into rows: numbered 1..N, no tint. The shape every
 *  whole-file caller wants, so neither of them restates the `i + 1`. */
export function fileRows(lines: string[]): CodeRow[] {
  return lines.map((html, i) => ({ html, num: i + 1 }));
}

/** Everything about one row that depends on the selection mode and the current
 *  selection. Kept as data rather than inlined in the JSX so the whole table is
 *  checkable without a DOM. */
export interface RenderedRow {
  key: string | number;
  cls: string;
  /** What the gutter shows: the line number, or nothing for a filler row. */
  gutter: string;
  /** `data-line`, which is what the scroll target looks a row up by. Absent for
   *  a filler row: nothing can scroll to a row that is not a line. */
  dataLine: number | undefined;
  /** The line a gutter click selects, or null when the click is inert (a filler
   *  row, or a column that does not participate in the file selection). */
  selectLine: number | null;
  html: string;
}

/** Resolve every row for rendering.
 *
 *  `selectable` gates BOTH the live gutter and the painted highlight, and `sel`
 *  is the selection to paint. The two are separate parameters because a file
 *  view with nothing selected still has a clickable gutter, and `selectable`
 *  gates the highlight as well as the click so the two cannot half-fail: the
 *  caller already passes `sel: null` for a non-participating column (that is
 *  what keeps it from subscribing to `selectedLines` at all), and this makes a
 *  selection handed in by mistake paint nothing anyway. */
export function renderRows(
  rows: CodeRow[],
  sel: { start: number; end: number } | null,
  selectable: boolean,
): RenderedRow[] {
  return rows.map((row, i) => {
    const isSelected = selectable && sel !== null && row.num !== null
      && row.num >= sel.start && row.num <= sel.end;
    return {
      key: row.num ?? `filler-${i}`,
      cls: ['code-line', row.cls, isSelected ? 'line-selected' : ''].filter(Boolean).join(' '),
      gutter: row.num === null ? '' : String(row.num),
      dataLine: row.num ?? undefined,
      selectLine: selectable ? row.num : null,
      html: row.html,
    };
  });
}

/** The line-numbered source view: numbered rows, click to select a line,
 *  shift-click to extend the range, and the selection rendered as a highlight.
 *
 *  Shared by both file previews (`RepoFilePreview` for a registered repository
 *  clone, `FilePreviewInline` for a workspace data file) and by each column of
 *  the side-by-side diff, so there is exactly one implementation of line
 *  numbering, selection and the navigate-driven scroll. The selection itself
 *  lives in the `selectedLines` store signal, which is also what
 *  `currentChatContext` reads to attach a line range to a chat message, so a
 *  range picked here is the same range a message carries.
 *
 *  Callers render this inside their own scroll container: `.repo-file-content`
 *  for the repo preview, `.file-preview-content` for the data-file one,
 *  `.side-by-side-diff-side` for a diff column. */
export function LineNumberedCode({ rows, selection = 'file' }: Props) {
  const preRef = useRef<HTMLPreElement>(null);
  const selectable = selection === 'file';

  const handleLineClick = useCallback((lineNum: number, shiftKey: boolean) => {
    if (shiftKey && selectedLines.value) {
      selectedLines.value = {
        start: Math.min(selectedLines.value.start, lineNum),
        end: Math.max(selectedLines.value.end, lineNum),
      };
    } else {
      selectedLines.value = { start: lineNum, end: lineNum };
    }
  }, []);

  // Honour a pending navigate-to-a-line request. `useSignalEffect` (not a plain
  // `useEffect`) so it fires on mount, which is the usual case where the request
  // was set while this file's content was still being fetched, AND on a later
  // request for a file already on screen, where nothing else about the render
  // would have changed. Consuming clears the request, so re-renders don't
  // re-scroll a user who has since scrolled away; a line past the end of the
  // file consumes it too and simply scrolls nowhere.
  useSignalEffect(() => {
    if (!selectable) return;
    const target = consumeLineScrollTarget();
    if (target === null) return;
    const row = preRef.current?.querySelector(`[data-line="${target}"]`);
    if (!row) {
      // The navigate named a line this file does not have, which is how a
      // citation decays. Nothing is highlighted, so nothing may be reported as
      // selected either: `currentChatContext` would otherwise attach a range
      // naming no code. This is the point where the line count is finally
      // known, so it is the only place that can tell.
      selectedLines.value = null;
      return;
    }
    row.scrollIntoView({ block: 'center' });
  });

  // Read CONDITIONALLY: reading a signal during render is what subscribes the
  // component to it, so a non-selectable column never subscribes and never
  // re-renders on a selection change elsewhere.
  const sel = selectable ? selectedLines.value : null;

  return (
    <pre
      class={`file-preview-code line-numbered${selectable ? '' : ' line-numbered-static'}`}
      ref={preRef}
    >
      {renderRows(rows, sel, selectable).map((r) => (
        <div key={r.key} data-line={r.dataLine} class={r.cls}>
          <span
            class="line-number"
            onClick={r.selectLine === null
              ? undefined
              : (e: MouseEvent) => handleLineClick(r.selectLine!, e.shiftKey)}
          >
            {r.gutter}
          </span>
          <span class="line-content" dangerouslySetInnerHTML={{ __html: r.html || ' ' }} />
        </div>
      ))}
    </pre>
  );
}
