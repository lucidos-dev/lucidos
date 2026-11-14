// @vitest-environment jsdom
/** Pan mode lays a file out as one gutter column beside one code column, so a
 *  long file carries a single pinned box. Wrap and caller modes keep each
 *  number inside its row. This pins the markup both rest on; the CSS half is
 *  `styles/__tests__/file-preview-line-wrapping.test.ts`. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

import { LineNumberedCode, fileRows, scrollIntoViewVertically, type WideLineMode } from '../LineNumberedCode';
import { PreviewPathContext, PreviewViewStateContext } from '../previewViewState';
import { createPreviewViewState, lineScrollTarget, selectedLines } from '../../../store/store';

let host: HTMLDivElement;

/** The file every preview here shows, as its host provides it. */
const PATH = 'artifacts/notes.md';
const at = (line: number) => ({ path: PATH, line });

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  selectedLines.value = null;
});

function show(wideLines: WideLineMode, lines = ['one', 'two', 'three', 'four']) {
  act(() => {
    render(
      <PreviewPathContext.Provider value={PATH}>
        <LineNumberedCode rows={fileRows(lines)} wideLines={wideLines} />
      </PreviewPathContext.Provider>,
      host,
    );
  });
  return host.querySelector('pre')!;
}

const texts = (els: NodeListOf<Element>) => Array.from(els, el => el.textContent);
const numbers = (els: NodeListOf<Element>) => Array.from(els, el => el.getAttribute('data-line-number'));

describe('pan mode', () => {
  it('puts every number in one gutter column, and no number in a row', () => {
    const pre = show('pan');
    expect(Array.from(pre.children, c => c.className)).toEqual(['line-numbered-gutter', 'line-numbered-code']);
    expect(numbers(pre.querySelectorAll('.line-numbered-gutter > .line-number'))).toEqual(['1', '2', '3', '4']);
    expect(pre.querySelectorAll('.code-line .line-number')).toHaveLength(0);
  });

  // Unhidden, a screen reader would read every number before the first line.
  it('hides the gutter column from assistive tech', () => {
    expect(show('pan').querySelector('.line-numbered-gutter')!.getAttribute('aria-hidden')).toBe('true');
  });

  it('keeps data-line on the code rows, in file order, for the scroll target', () => {
    const pre = show('pan');
    const rows = pre.querySelectorAll('.line-numbered-code > .code-line');
    expect(Array.from(rows, r => r.getAttribute('data-line'))).toEqual(['1', '2', '3', '4']);
    expect(texts(pre.querySelectorAll('.code-line .line-content'))).toEqual(['one', 'two', 'three', 'four']);
    expect(pre.querySelectorAll('[data-line]')).toHaveLength(4);
  });

  it('selects from the gutter, and tints both the cell and the row', () => {
    const pre = show('pan');
    const cells = pre.querySelectorAll<HTMLElement>('.line-numbered-gutter .line-number');
    act(() => { cells[1].click(); });
    act(() => { cells[2].dispatchEvent(new MouseEvent('click', { bubbles: true, shiftKey: true })); });
    expect(selectedLines.value).toEqual({ start: 2, end: 3 });

    const selectedCells = pre.querySelectorAll('.line-numbered-gutter .line-selected');
    const selectedRows = pre.querySelectorAll('.line-numbered-code .line-selected');
    expect(numbers(selectedCells)).toEqual(['2', '3']);
    expect(Array.from(selectedRows, r => r.getAttribute('data-line'))).toEqual(['2', '3']);
  });
});

/** A row wider than the view, scrolled into view itself, has its left edge
 *  panned to the scrollport's, under the pinned gutter. So a cited line opens
 *  with its first characters hidden. Its gutter cell never moves sideways. */
describe('the scroll target in pan mode', () => {
  afterEach(() => { lineScrollTarget.value = null; });

  function scrolledOnOpen(wideLines: WideLineMode): Element[] {
    const scrolled: Element[] = [];
    const spy = vi.spyOn(Element.prototype, 'scrollIntoView')
      .mockImplementation(function (this: Element) { scrolled.push(this); });
    lineScrollTarget.value = at(3);
    show(wideLines);
    spy.mockRestore();
    return scrolled;
  }

  it('scrolls to the cited line by its gutter cell', () => {
    const [target] = scrolledOnOpen('pan');
    expect(target.classList.contains('line-number')).toBe(true);
    expect(target.parentElement!.classList.contains('line-numbered-gutter')).toBe(true);
    expect(target.getAttribute('data-line-number')).toBe('3');
  });

  it('still scrolls the row itself in wrap mode', () => {
    const [target] = scrolledOnOpen('wrap');
    expect(target.getAttribute('data-line')).toBe('3');
  });
});

/** A navigate moves the view up or down only. `scrollIntoView` can also pan an
 *  ancestor sideways, which opened the file preview modal with the start of
 *  every line cut off. */
describe('the navigate scroll is vertical only', () => {
  it('puts back every ancestor scrollLeft that scrollIntoView moved', () => {
    const outer = document.createElement('div');
    const inner = document.createElement('div');
    const row = document.createElement('div');
    outer.appendChild(inner);
    inner.appendChild(row);
    host.appendChild(outer);
    inner.scrollLeft = 0;
    let pannedTo: number | null = null;
    const spy = vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(() => {
      inner.scrollLeft = 40;
      outer.scrollLeft = 12;
      pannedTo = inner.scrollLeft;
    });
    scrollIntoViewVertically(row);
    expect(spy).toHaveBeenCalledWith({ block: 'center' });
    spy.mockRestore();
    expect(pannedTo, 'the pan must stick, or the restore proves nothing').toBe(40);
    expect(inner.scrollLeft).toBe(0);
    expect(outer.scrollLeft).toBe(0);
  });
});

/** The file preview modal can open over a Files preview. Each reads the view
 *  state its provider gives it, so neither paints or scrolls for the other. */
describe('each preview reads its own view state', () => {
  afterEach(() => { lineScrollTarget.value = null; });

  it('keeps the modal and the panel apart', () => {
    const modalView = createPreviewViewState();
    modalView.selectedLines.value = { start: 2, end: 2 };
    modalView.lineScrollTarget.value = at(2);
    selectedLines.value = { start: 4, end: 4 };
    lineScrollTarget.value = at(4);
    const scrolled: Element[] = [];
    const spy = vi.spyOn(Element.prototype, 'scrollIntoView')
      .mockImplementation(function (this: Element) { scrolled.push(this); });
    const rows = fileRows(['one', 'two', 'three', 'four']);
    act(() => {
      render(
        <PreviewPathContext.Provider value={PATH}>
          <div data-role="panel"><LineNumberedCode rows={rows} wideLines="wrap" /></div>
          <PreviewViewStateContext.Provider value={modalView}>
            <div data-role="modal"><LineNumberedCode rows={rows} wideLines="wrap" /></div>
          </PreviewViewStateContext.Provider>
        </PreviewPathContext.Provider>,
        host,
      );
    });
    spy.mockRestore();

    const selected = (role: string) => Array.from(
      host.querySelectorAll(`[data-role="${role}"] .line-selected`), el => el.getAttribute('data-line'));
    expect(selected('panel')).toEqual(['4']);
    expect(selected('modal')).toEqual(['2']);
    expect(scrolled.map(el => el.closest('[data-role]')!.getAttribute('data-role') + ':' + el.getAttribute('data-line')))
      .toEqual(['panel:4', 'modal:2']);
    expect(lineScrollTarget.value).toBeNull();
    expect(modalView.lineScrollTarget.value).toBeNull();
  });

  // A second preview of the same file replaces the first: the code stays
  // mounted, but its provider hands it a new view with a new target.
  it('scrolls for a replacing view on the same mounted code', () => {
    const rows = fileRows(['one', 'two', 'three', 'four']);
    const scrolled: (string | null)[] = [];
    const spy = vi.spyOn(Element.prototype, 'scrollIntoView')
      .mockImplementation(function (this: Element) { scrolled.push(this.getAttribute('data-line')); });
    const show = (view: ReturnType<typeof createPreviewViewState>) => act(() => {
      render(
        <PreviewViewStateContext.Provider value={view}>
          <PreviewPathContext.Provider value={PATH}>
            <LineNumberedCode rows={rows} wideLines="wrap" />
          </PreviewPathContext.Provider>
        </PreviewViewStateContext.Provider>,
        host,
      );
    });
    const first = createPreviewViewState();
    first.lineScrollTarget.value = at(2);
    show(first);
    const second = createPreviewViewState();
    second.lineScrollTarget.value = at(4);
    show(second);
    spy.mockRestore();
    expect(scrolled).toEqual(['2', '4']);
    expect(second.lineScrollTarget.value).toBeNull();
  });
});

/** A navigate to a line in another file writes its target while this file's
 *  rows are still mounted, before the new file renders. Taking it here scrolled
 *  the wrong file, and told Find in file the landing was done before the new
 *  file had any rows to search. */
describe('the scroll target belongs to the file it names', () => {
  afterEach(() => { lineScrollTarget.value = null; });

  it('leaves a target for another file to that file', () => {
    const scrolled = vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(() => {});
    show('wrap');
    const elsewhere = { path: 'artifacts/log.txt', line: 3 };
    act(() => { lineScrollTarget.value = elsewhere; });
    expect(scrolled).not.toHaveBeenCalled();
    expect(lineScrollTarget.value).toEqual(elsewhere);
    expect(selectedLines.value, 'nor does it decide the line is missing').toBeNull();
    scrolled.mockRestore();
  });

  it('takes a target for its own file once it is on screen', () => {
    const scrolled: (string | null)[] = [];
    const spy = vi.spyOn(Element.prototype, 'scrollIntoView')
      .mockImplementation(function (this: Element) { scrolled.push(this.getAttribute('data-line')); });
    show('wrap');
    act(() => { lineScrollTarget.value = at(3); });
    spy.mockRestore();
    expect(scrolled).toEqual(['3']);
    expect(lineScrollTarget.value).toBeNull();
  });
});

describe('wrap mode keeps each number inside its row', () => {
  it('has no gutter column', () => {
    const pre = show('wrap');
    expect(pre.querySelector('.line-numbered-gutter')).toBeNull();
    expect(numbers(pre.querySelectorAll(':scope > .code-line > .line-number'))).toEqual(['1', '2', '3', '4']);
  });
});

/** Find in file walks the view's text nodes. A number held as text would match
 *  a numeric search, and in pan mode every gutter hit would come before the
 *  code. So the number is generated content, and the only text is the code. */
describe('the line numbers are not text', () => {
  for (const mode of ['pan', 'wrap', 'caller'] as const) {
    it(`leaves only the code as text in ${mode} mode`, () => {
      expect(show(mode, ['alpha', 'beta']).textContent).toBe('alphabeta');
    });
  }
});
