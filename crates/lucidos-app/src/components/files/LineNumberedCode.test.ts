import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { fileRows, renderRows } from './LineNumberedCode';

describe('fileRows', () => {
  it('numbers a file 1..N', () => {
    expect(fileRows(['a', 'b', 'c'])).toEqual([
      { html: 'a', num: 1 },
      { html: 'b', num: 2 },
      { html: 'c', num: 3 },
    ]);
  });

  it('has no rows for an empty file', () => {
    expect(fileRows([])).toEqual([]);
  });
});

describe('renderRows: the file view', () => {
  const rows = fileRows(['one', 'two', 'three', 'four']);

  it('paints the selected range and nothing outside it', () => {
    const rendered = renderRows(rows, { start: 2, end: 3 }, true);
    expect(rendered.map(r => r.cls.includes('line-selected'))).toEqual([false, true, true, false]);
  });

  it('numbers the gutter and exposes data-line for the scroll target', () => {
    const rendered = renderRows(rows, null, true);
    expect(rendered.map(r => r.gutter)).toEqual(['1', '2', '3', '4']);
    expect(rendered.map(r => r.dataLine)).toEqual([1, 2, 3, 4]);
  });

  it('keeps the gutter clickable with nothing selected', () => {
    expect(renderRows(rows, null, true).map(r => r.selectLine)).toEqual([1, 2, 3, 4]);
  });

  it('carries a caller class alongside the selection class', () => {
    const rendered = renderRows([{ html: 'x', num: 1, cls: 'side-by-side-diff-addition' }], { start: 1, end: 1 }, true);
    expect(rendered[0].cls).toBe('code-line side-by-side-diff-addition line-selected');
  });
});

/** A side-by-side diff column numbers the OLD file on the left and the NEW file on the
 *  right, so one file-level selection cannot mean both. All three behaviours are
 *  off there: the click, the highlight, and (below) the scroll-target consume. */
describe('renderRows: a non-participating column', () => {
  const rows = fileRows(['one', 'two', 'three']);

  it('makes every gutter click inert', () => {
    expect(renderRows(rows, null, false).map(r => r.selectLine)).toEqual([null, null, null]);
  });

  // The caller passes `sel: null` for a column precisely so it never subscribes
  // to `selectedLines`; this pins that even a selection handed in by mistake
  // paints nothing, so the two gates cannot half-fail.
  it('paints no highlight even for a matching selection', () => {
    const rendered = renderRows(rows, { start: 1, end: 3 }, false);
    expect(rendered.every(r => !r.cls.includes('line-selected'))).toBe(true);
  });
});

/** A filler row exists only to keep the two columns of a side-by-side diff lined up
 *  where one side has no line. It is not a line, so it shows no number, is not
 *  a scroll target, and is never selectable. */
describe('renderRows: a filler row', () => {
  it('shows no number, exposes no data-line, and cannot be clicked', () => {
    const [filler] = renderRows([{ html: '', num: null }], { start: 1, end: 99 }, true);
    expect(filler.gutter).toBe('');
    expect(filler.dataLine).toBeUndefined();
    expect(filler.selectLine).toBeNull();
    expect(filler.cls).not.toContain('line-selected');
  });

  it('keys distinctly from its neighbours so a run of them does not collapse', () => {
    const keys = renderRows(
      [{ html: '', num: null }, { html: '', num: null }, { html: 'x', num: 7 }],
      null,
      true,
    ).map(r => r.key);
    expect(new Set(keys).size).toBe(3);
  });
});

/** The third gate lives in a `useSignalEffect` and is DOM-bound, so it cannot be
 *  driven from here. It matters as much as the other two: a column that consumed
 *  a pending `lineScrollTarget` would swallow a navigate meant for a file view
 *  and, finding no such row, null the selection out from under it. */
describe('the scroll target is consumed only by a participating view', () => {
  const source = readFileSync(new URL('./LineNumberedCode.tsx', import.meta.url), 'utf8');

  it('gates the consume on the same selectable flag as the click', () => {
    const effect = source.split('useSignalEffect(')[1];
    expect(effect).toBeDefined();
    const beforeConsume = effect.split('consumeLineScrollTarget')[0];
    expect(beforeConsume).toContain('selectable');
  });

  it('consumes the target in exactly one place', () => {
    expect(source.match(/consumeLineScrollTarget\(/g)).toHaveLength(1);
  });
});
