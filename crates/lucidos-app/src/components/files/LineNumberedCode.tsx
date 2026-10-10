import { useCallback, useEffect, useRef } from 'preact/hooks';
import { effect } from '@preact/signals';
import type { VNode } from 'preact';
import { consumeLineScrollTarget } from '../../store/store';
import { usePreviewPath, usePreviewViewState } from './previewViewState';
import { SkText, useSkeleton } from '../shared/Skeleton';

/** Line widths the source view shimmers with while its file loads. */
const SKELETON_LINE_WIDTHS = ['46%', '72%', '58%', '84%', '30%', '66%', '52%', '78%', '40%', '62%'];

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
 *  preview's `selectedLines`, that selection renders as a highlight here, and a
 *  pending `lineScrollTarget` is consumed and scrolled to here.
 *
 *  `none` means they are a derived view: one column of a side-by-side diff, whose
 *  numbers are the OLD file's on the left and the NEW file's on the right. A
 *  file-level selection cannot mean both, so all three behaviours are off.
 *  Gating the scroll consumption matters as much as gating the click: a column
 *  that consumed a pending target would swallow a navigate meant for a file
 *  view, and (finding no such row) null the selection out from under it. */
export type LineSelectionMode = 'file' | 'none';

/** What a line too wide for the box does.
 *
 *  `wrap`   soft-wrap it. A file preview's default, because a clipped tail is
 *           unreachable and a reader following a citation needs the part it
 *           was about.
 *  `pan`    keep it on one line and pan it, under a gutter pinned to the left
 *           edge. The gutter is ONE column beside the code, so a long file
 *           carries one pinned box rather than one per line.
 *  `caller` neither. The caller's own scroller owns the overflow, and its own
 *           rules own the row's background.
 *
 *  `caller` is what a side-by-side diff column passes, and the distinction is
 *  load-bearing rather than tidy. Those columns tint rows green and red, and
 *  their gutter must carry the tint with its row, which a separate pinned
 *  column would not. */
export type WideLineMode = 'wrap' | 'pan' | 'caller';

interface Props {
  rows: CodeRow[];
  selection?: LineSelectionMode;
  /** Required rather than defaulted: the three modes make different promises,
   *  so a new caller has to pick one. */
  wideLines: WideLineMode;
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
  selected: boolean;
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
      selected: isSelected,
      gutter: row.num === null ? '' : String(row.num),
      dataLine: row.num ?? undefined,
      selectLine: selectable ? row.num : null,
      html: row.html,
    };
  });
}

/** Every class on the `<pre>`, given the mode it renders in.
 *
 *  `line-numbered-wrap` and `line-numbered-pan` are mutually exclusive, so
 *  neither mode's rules can reach a `<pre>` the other owns. `caller` carries
 *  neither, which is what keeps both sets off a diff column. Pure and exported
 *  so the pairing is checkable without a DOM. */
export function codeBlockClass(selectable: boolean, wideLines: WideLineMode): string {
  const mode = wideLines === 'caller' ? '' : `line-numbered-${wideLines}`;
  return [
    'file-preview-code',
    'line-numbered',
    selectable ? '' : 'line-numbered-static',
    mode,
  ].filter(Boolean).join(' ');
}

/** The line-numbered source view: numbered rows, click to select a line,
 *  shift-click to extend the range, and the selection rendered as a highlight.
 *
 *  Shared by both file previews (`RepoFilePreview` for a registered repository
 *  clone, `FilePreviewInline` for a workspace data file) and by each column of
 *  the side-by-side diff. So there is exactly one implementation of line
 *  numbering, selection and the navigate-driven scroll. The selection lives in
 *  the provided `PreviewViewState`. In the Files panel that is the global
 *  `selectedLines`. `currentChatContext` attaches it to a chat message, so a
 *  range picked there is the range a message carries.
 *
 *  Callers render this inside their own scroll container: `.repo-file-content`
 *  for the repo preview, `.file-preview-content` for the data-file one,
 *  `.side-by-side-diff-side` for a diff column. */
export function LineNumberedCode({ rows, selection = 'file', wideLines }: Props) {
  const preRef = useRef<HTMLPreElement>(null);
  const sk = useSkeleton();
  const { selectedLines, lineScrollTarget } = usePreviewViewState();
  const path = usePreviewPath();
  // A placeholder is never a file view: it must not consume a scroll target
  // that belongs to the content it stands in for.
  const selectable = selection === 'file' && !sk;

  const handleLineClick = useCallback((lineNum: number, shiftKey: boolean) => {
    if (shiftKey && selectedLines.value) {
      selectedLines.value = {
        start: Math.min(selectedLines.value.start, lineNum),
        end: Math.max(selectedLines.value.end, lineNum),
      };
    } else {
      selectedLines.value = { start: lineNum, end: lineNum };
    }
  }, [selectedLines]);

  // Honour a pending navigate-to-a-line request. A signal `effect` fires on
  // mount, the usual case: the request was set while the content was loading.
  // It also fires on a later request for a file already on screen, where
  // nothing else about the render changed. It re-subscribes when the provided
  // view or path changes, since a replacing modal preview of the same file keeps
  // this component mounted. Only a target naming this file is taken. The router
  // writes it while the previous file's rows are still mounted. Consuming
  // clears the request, so a re-render never re-scrolls a reader who scrolled
  // away. A line past the end scrolls nowhere.
  useEffect(() => effect(() => {
    if (!selectable) return;
    const target = consumeLineScrollTarget(lineScrollTarget, path);
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
    // In pan mode the row starts right of the pinned gutter. Scrolling a row
    // wider than the view would pan its left edge under the gutter. Its gutter
    // cell is level with it and never off the left edge.
    const gutter = preRef.current!.querySelector('.line-numbered-gutter');
    const cell = gutter?.children[Array.prototype.indexOf.call(row.parentElement!.children, row)];
    scrollIntoViewVertically(cell ?? row);
  }), [selectable, lineScrollTarget, selectedLines, path]);

  // Read CONDITIONALLY: reading a signal during render is what subscribes the
  // component to it, so a non-selectable column never subscribes and never
  // re-renders on a selection change elsewhere.
  const sel = selectable ? selectedLines.value : null;

  const pan = wideLines === 'pan';

  if (sk) {
    return (
      <pre class={codeBlockClass(false, wideLines)} aria-hidden="true">
        {layoutCells(pan, SKELETON_LINE_WIDTHS.map((w, i) => ({
          key: i,
          cls: 'code-line',
          selected: false,
          dataLine: undefined,
          gutter: String(i + 1),
          onGutterClick: undefined,
          content: <SkText class="line-content" w={w} />,
        })))}
      </pre>
    );
  }

  return (
    <pre
      class={codeBlockClass(selectable, wideLines)}
      ref={preRef}
    >
      {layoutCells(pan, renderRows(rows, sel, selectable).map((r) => ({
        ...r,
        onGutterClick: r.selectLine === null
          ? undefined
          : (e: MouseEvent) => handleLineClick(r.selectLine!, e.shiftKey),
        content: <span class="line-content" dangerouslySetInnerHTML={{ __html: r.html || ' ' }} />,
      })))}
    </pre>
  );
}

/** One row's gutter and content, before `layoutCells` places them. */
interface RowCells {
  key: string | number;
  cls: string;
  selected: boolean;
  dataLine: number | undefined;
  gutter: string;
  onGutterClick: ((e: MouseEvent) => void) | undefined;
  content: VNode;
}

/** Place the cells. Wrap and caller modes keep each gutter cell inside its
 *  row, since a wrapped row grows and a diff row's tint must cover its number.
 *
 *  Pan mode splits them into a gutter column and a code column, and pins only
 *  the column. A sticky cell per row made WebKit reposition every one of them
 *  on each scroll frame, about 48 ms a frame at 2,600 lines. Every row is one
 *  line tall in pan mode, so cell N and row N stay level without measuring.
 *  `data-line` stays on the code row, which is what the scroll target finds.
 *  The column is hidden from assistive tech, which would otherwise read every
 *  number before the first line of code.
 *
 *  In every mode the number is CSS generated content from `data-line-number`,
 *  never a text node. So find in file matches only code, and a copied
 *  selection carries no numbers. */
function layoutCells(pan: boolean, cells: RowCells[]) {
  const gutterCell = (c: RowCells) => (
    <span
      key={c.key}
      class={pan && c.selected ? 'line-number line-selected' : 'line-number'}
      data-line-number={c.gutter || undefined}
      onClick={c.onGutterClick}
    />
  );
  if (!pan) {
    return cells.map((c) => (
      <div key={c.key} data-line={c.dataLine} class={c.cls}>
        {gutterCell(c)}
        {c.content}
      </div>
    ));
  }
  return [
    <div key="gutter" class="line-numbered-gutter" aria-hidden="true">{cells.map(gutterCell)}</div>,
    <div key="code" class="line-numbered-code">
      {cells.map((c) => <div key={c.key} data-line={c.dataLine} class={c.cls}>{c.content}</div>)}
    </div>,
  ];
}

/** Center `el` vertically, leaving every ancestor's horizontal scroll where it
 *  was. `scrollIntoView` can also pan: aimed at the pinned gutter cell, it
 *  opened the file preview modal with the start of every line cut off. */
export function scrollIntoViewVertically(el: Element): void {
  const panned: [Element, number][] = [];
  for (let node = el.parentElement; node; node = node.parentElement) {
    panned.push([node, node.scrollLeft]);
  }
  el.scrollIntoView({ block: 'center' });
  for (const [node, left] of panned) {
    if (node.scrollLeft !== left) node.scrollLeft = left;
  }
}
