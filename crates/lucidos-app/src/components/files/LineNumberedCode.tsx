import { useCallback, useEffect, useRef } from 'preact/hooks';
import { effect } from '@preact/signals';
import type { VNode } from 'preact';
import { consumeLineScrollTarget } from '../../store/store';
import { usePreviewPath, usePreviewViewState } from './previewViewState';
import { SkText, useSkeleton } from '../shared/Skeleton';

const SKELETON_LINE_WIDTHS = ['46%', '72%', '58%', '84%', '30%', '66%', '52%', '78%', '40%', '62%'];

export interface CodeRow {
  html: string;
  num: number | null;
  cls?: string;
}

export type LineSelectionMode = 'file' | 'none';

export type WideLineMode = 'wrap' | 'pan' | 'caller';

interface Props {
  rows: CodeRow[];
  selection?: LineSelectionMode;
  wideLines: WideLineMode;
}

export function fileRows(lines: string[]): CodeRow[] {
  return lines.map((html, i) => ({ html, num: i + 1 }));
}

export interface RenderedRow {
  key: string | number;
  cls: string;
  selected: boolean;
  gutter: string;
  dataLine: number | undefined;
  selectLine: number | null;
  html: string;
}

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

export function codeBlockClass(selectable: boolean, wideLines: WideLineMode): string {
  const mode = wideLines === 'caller' ? '' : `line-numbered-${wideLines}`;
  return [
    'file-preview-code',
    'line-numbered',
    selectable ? '' : 'line-numbered-static',
    mode,
  ].filter(Boolean).join(' ');
}

export function LineNumberedCode({ rows, selection = 'file', wideLines }: Props) {
  const preRef = useRef<HTMLPreElement>(null);
  const sk = useSkeleton();
  const { selectedLines, lineScrollTarget } = usePreviewViewState();
  const path = usePreviewPath();
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

  useEffect(() => effect(() => {
    if (!selectable) return;
    const target = consumeLineScrollTarget(lineScrollTarget, path);
    if (target === null) return;
    const row = preRef.current?.querySelector(`[data-line="${target}"]`);
    if (!row) {
      selectedLines.value = null;
      return;
    }
    const gutter = preRef.current!.querySelector('.line-numbered-gutter');
    const cell = gutter?.children[Array.prototype.indexOf.call(row.parentElement!.children, row)];
    scrollIntoViewVertically(cell ?? row);
  }), [selectable, lineScrollTarget, selectedLines, path]);

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

interface RowCells {
  key: string | number;
  cls: string;
  selected: boolean;
  dataLine: number | undefined;
  gutter: string;
  onGutterClick: ((e: MouseEvent) => void) | undefined;
  content: VNode;
}

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
