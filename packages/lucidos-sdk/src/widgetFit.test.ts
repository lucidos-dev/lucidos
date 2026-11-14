// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MAX_FIT_FINDINGS, SETTLE_CAP_MS, SETTLE_QUIET_MS, measureFit, settle, type FitGeometry, type Rect } from './widgetFit';

const rect = (left: number, top: number, width: number, height: number): Rect =>
  ({ left, top, right: left + width, bottom: top + height });

/** A box of 300 x 200, with each element's and text's rect named by `data-r`. */
function geometryFor(rects: Record<string, Rect>, box = { width: 300, height: 200 }): FitGeometry {
  const named = (el: Element | null) => (el ? rects[el.getAttribute('data-r') ?? ''] : undefined);
  return {
    box: () => box,
    rect: (el) => named(el) ?? rect(0, 0, 0, 0),
    textRect: (text) => named(text.parentElement) ?? null,
  };
}

describe('measureFit', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('finds nothing in content that sits inside the box', () => {
    document.body.innerHTML = '<div class="row" data-r="row"><span data-r="a">Oslo to Rome</span></div>';
    const fit = measureFit(document, geometryFor({ row: rect(0, 0, 300, 20), a: rect(0, 0, 120, 20) }));
    expect(fit).toEqual({ findings: [], total: 0 });
  });

  it('reports text past the right edge with its edge, px, path and text', () => {
    document.body.innerHTML = '<div class="row" data-r="row"><span data-r="a">Oslo Gardermoen to Rome Fiumicino</span></div>';
    const fit = measureFit(document, geometryFor({ row: rect(0, 0, 300, 20), a: rect(0, 0, 342.4, 20) }));
    expect(fit.findings).toEqual([{
      kind: 'past-edge', edge: 'right', px: 42, path: 'div.row > span', text: 'Oslo Gardermoen to Rome Fiumicino',
    }]);
  });

  it('reports the edge text runs furthest past', () => {
    document.body.innerHTML = '<p data-r="a">Long tail</p>';
    const fit = measureFit(document, geometryFor({ a: rect(-5, 150, 320, 90) }));
    expect(fit.findings[0]).toMatchObject({ edge: 'bottom', px: 40 });
  });

  it('allows one pixel of slack for a fractional layout', () => {
    document.body.innerHTML = '<p data-r="a">Nearly</p>';
    expect(measureFit(document, geometryFor({ a: rect(0, 0, 300.9, 200.9) })).findings).toEqual([]);
  });

  it('reports text that an overflow-hidden ancestor cuts, inside the box', () => {
    document.body.innerHTML = '<div class="cell" data-r="cell" style="overflow-x: hidden"><b data-r="a">Departs 07:45 from gate 12</b></div>';
    const fit = measureFit(document, geometryFor({ cell: rect(0, 0, 100, 20), a: rect(0, 0, 180, 20) }));
    expect(fit.findings).toEqual([{ kind: 'clipped', edge: 'right', px: 80, path: 'div.cell > b', text: 'Departs 07:45 from gate 12' }]);
  });

  it('reports a clip ancestor too', () => {
    document.body.innerHTML = '<div data-r="cell" style="overflow-y: clip"><i data-r="a">Row</i></div>';
    const fit = measureFit(document, geometryFor({ cell: rect(0, 0, 100, 20), a: rect(0, 0, 50, 34) }));
    expect(fit.findings).toEqual([{ kind: 'clipped', edge: 'bottom', px: 14, path: 'div > i', text: 'Row' }]);
  });

  it('reports a cut only on the axis an ancestor clips', () => {
    document.body.innerHTML = '<div data-r="cell" style="overflow-x: clip"><i data-r="a">Row</i></div>';
    const tall = geometryFor({ cell: rect(0, 0, 100, 20), a: rect(0, 0, 50, 60) });
    expect(measureFit(document, tall).findings, 'its bottom is not clipped').toEqual([]);
    const both = geometryFor({ cell: rect(0, 0, 100, 20), a: rect(0, 0, 130, 60) });
    expect(measureFit(document, both).findings).toEqual([{ kind: 'clipped', edge: 'right', px: 30, path: 'div > i', text: 'Row' }]);
  });

  it('never calls a scroller a cut: the user can reach its content', () => {
    document.body.innerHTML = '<div data-r="cell" style="overflow-x: auto"><i data-r="a">Row</i></div>';
    expect(measureFit(document, geometryFor({ cell: rect(0, 0, 100, 20), a: rect(0, 0, 180, 20) })).findings).toEqual([]);
  });

  it('reports an ellipsis cut once, and not again as a clip', () => {
    document.body.innerHTML =
      '<ul><li class="flight" data-r="li" style="overflow-x: hidden; text-overflow: ellipsis"><span data-r="s">SK4711 Oslo to Rome</span>, departs 07:45</li></ul>';
    const li = document.querySelector('li')!;
    Object.defineProperty(li, 'scrollWidth', { configurable: true, value: 260 });
    Object.defineProperty(li, 'clientWidth', { configurable: true, value: 140 });
    const fit = measureFit(document, geometryFor({ li: rect(0, 0, 140, 20), s: rect(0, 0, 200, 20) }));
    expect(fit.findings).toEqual([{
      kind: 'ellipsis', edge: 'right', px: 120, path: 'ul > li.flight', text: 'SK4711 Oslo to Rome, departs 07:45',
    }]);
  });

  it('an ellipsis that has room cuts nothing', () => {
    document.body.innerHTML = '<p data-r="p" style="text-overflow: ellipsis">Short</p>';
    const p = document.querySelector('p')!;
    Object.defineProperty(p, 'scrollWidth', { configurable: true, value: 80 });
    Object.defineProperty(p, 'clientWidth', { configurable: true, value: 80 });
    expect(measureFit(document, geometryFor({ p: rect(0, 0, 80, 20) })).findings).toEqual([]);
  });

  it('reports an image, svg, canvas or video past an edge', () => {
    document.body.innerHTML = '<img data-r="img" alt="map"><svg data-r="svg"></svg><canvas data-r="c"></canvas><video data-r="v"></video>';
    const fit = measureFit(document, geometryFor({
      img: rect(0, 0, 330, 10), svg: rect(0, 180, 10, 30), c: rect(0, 0, 10, 10), v: rect(-12, 0, 40, 10),
    }));
    expect(fit.findings).toEqual([
      { kind: 'media', edge: 'right', px: 30, path: 'img', text: 'map' },
      { kind: 'media', edge: 'bottom', px: 10, path: 'svg', text: '' },
      { kind: 'media', edge: 'left', px: 12, path: 'video', text: '' },
    ]);
  });

  it('skips text that draws nothing', () => {
    document.body.innerHTML = '<p data-r="a" style="visibility: hidden">Hidden</p><p data-r="b">   </p><p>No rect</p>';
    expect(measureFit(document, geometryFor({ a: rect(0, 0, 900, 20), b: rect(0, 0, 900, 20) })).findings).toEqual([]);
  });

  it('quotes the first 60 characters of text, whitespace collapsed', () => {
    const long = `${'Flight  details\n'.repeat(10)}`;
    document.body.innerHTML = `<p data-r="a">${long}</p>`;
    const fit = measureFit(document, geometryFor({ a: rect(0, 0, 400, 20) }));
    expect(fit.findings[0].text).toBe(long.replace(/\s+/g, ' ').trim().slice(0, 60));
  });

  it('keeps at most a few findings per box and counts the rest', () => {
    const rows = Array.from({ length: 11 }, (_, i) => `<p data-r="p${i}">Row ${i}</p>`).join('');
    document.body.innerHTML = rows;
    const rects = Object.fromEntries(Array.from({ length: 11 }, (_, i) => [`p${i}`, rect(0, 0, 400, 20)]));
    const fit = measureFit(document, geometryFor(rects));
    expect(fit.findings).toHaveLength(MAX_FIT_FINDINGS);
    expect(fit.total).toBe(11);
  });
});

describe('settle', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = '<p>Start</p>';
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  const settled = () => {
    let done = false;
    void settle(document).then(() => { done = true; });
    return () => done;
  };

  it('settles once the DOM holds still for the quiet window', async () => {
    const done = settled();
    await vi.advanceTimersByTimeAsync(SETTLE_QUIET_MS - 10);
    expect(done()).toBe(false);
    await vi.advanceTimersByTimeAsync(20);
    expect(done()).toBe(true);
  });

  it('a DOM change restarts the quiet window', async () => {
    const done = settled();
    await vi.advanceTimersByTimeAsync(SETTLE_QUIET_MS - 50);
    document.body.append('More');
    await vi.advanceTimersByTimeAsync(100);
    expect(done()).toBe(false);
    await vi.advanceTimersByTimeAsync(SETTLE_QUIET_MS);
    expect(done()).toBe(true);
  });

  it('a widget that never holds still settles at the cap', async () => {
    const done = settled();
    const ticker = setInterval(() => document.body.append('.'), 100);
    await vi.advanceTimersByTimeAsync(SETTLE_CAP_MS - 10);
    expect(done()).toBe(false);
    await vi.advanceTimersByTimeAsync(20);
    expect(done()).toBe(true);
    clearInterval(ticker);
  });

  it('waits for the fonts first, inside the same cap', async () => {
    let fontsLoaded: () => void = () => {};
    Object.defineProperty(document, 'fonts', {
      configurable: true,
      value: { ready: new Promise<void>((resolve) => { fontsLoaded = resolve; }) },
    });
    const done = settled();
    await vi.advanceTimersByTimeAsync(SETTLE_QUIET_MS * 2);
    expect(done()).toBe(false);
    fontsLoaded();
    await vi.advanceTimersByTimeAsync(SETTLE_QUIET_MS + 10);
    expect(done()).toBe(true);
    Reflect.deleteProperty(document, 'fonts');
  });
});
