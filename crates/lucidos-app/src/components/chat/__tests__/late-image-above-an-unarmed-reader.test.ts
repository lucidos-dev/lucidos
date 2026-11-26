import { describe, it, expect, beforeEach, afterEach } from 'vitest';

if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(0); return 0; };
  (globalThis as any).cancelAnimationFrame = () => {};
}

import { mockContainer, mockTranscript } from './scroll-test-helpers';
import {
  clearPendingEventScroll,
  makeScrollObservers,
  markNavigationScroll,
  readerGestureForTest,
  setFollowLiveEdge,
} from '../scrollState';

const IDS = ['t0', 't1', 't2', 't3', 't4', 't5'];
const T2_BLOCKS = [20, 100, 1000, 200];
const READ_BLOCK = 2;
const IMAGE_BLOCK = 0;
const IMAGE_GROWTH = 280;

describe('a late image above an unarmed reader', () => {
  let origRO: unknown;
  let origMO: unknown;

  beforeEach(() => {
    setFollowLiveEdge(false);
    clearPendingEventScroll();
    origRO = (globalThis as any).ResizeObserver;
    origMO = (globalThis as any).MutationObserver;
    class Inert { observe() {} disconnect() {} takeRecords() { return []; } }
    (globalThis as any).ResizeObserver = Inert;
    (globalThis as any).MutationObserver = Inert;
  });
  afterEach(() => {
    readerGestureForTest(null, false);
    setFollowLiveEdge(false);
    (globalThis as any).ResizeObserver = origRO;
    (globalThis as any).MutationObserver = origMO;
  });

  function readerRestsAt(scrollTop: number) {
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [...T2_BLOCKS] } });
    const observers = makeScrollObservers(el as unknown as HTMLElement);
    readerGestureForTest(el as unknown as HTMLElement);
    el.scrollTop = scrollTop;
    observers.onScroll();
    readerGestureForTest(null, false);
    return { el, ...observers };
  }

  it('holds the line when an image grows above it in the same turn', () => {
    const { el, onResize, detachGestures } = readerRestsAt(1320);
    const lineBefore = el.blockRelTop('t2', READ_BLOCK);

    el.setBlockHeight('t2', IMAGE_BLOCK, T2_BLOCKS[IMAGE_BLOCK] + IMAGE_GROWTH);
    onResize();

    expect(el.scrollTop).toBe(1320 + IMAGE_GROWTH);
    expect(el.blockRelTop('t2', READ_BLOCK)).toBe(lineBefore);
    detachGestures();
  });

  it('holds the line when a scroll event lands between the growth and its resize', () => {
    const { el, onScroll, onResize, detachGestures } = readerRestsAt(1320);
    const lineBefore = el.blockRelTop('t2', READ_BLOCK);

    el.setBlockHeight('t2', IMAGE_BLOCK, T2_BLOCKS[IMAGE_BLOCK] + IMAGE_GROWTH);
    onScroll();
    onResize();

    expect(el.scrollTop).toBe(1320 + IMAGE_GROWTH);
    expect(el.blockRelTop('t2', READ_BLOCK)).toBe(lineBefore);
    detachGestures();
  });

  it('holds the line when the scroll position settles one pixel off its record', () => {
    const { el, onResize, detachGestures } = readerRestsAt(1320);
    const lineBefore = el.blockRelTop('t2', READ_BLOCK);

    markNavigationScroll(el, 1321);
    el.setBlockHeight('t2', IMAGE_BLOCK, T2_BLOCKS[IMAGE_BLOCK] + IMAGE_GROWTH);
    onResize();

    expect(el.scrollTop).toBe(1320 + IMAGE_GROWTH);
    expect(el.blockRelTop('t2', READ_BLOCK)).toBe(lineBefore);
    detachGestures();
  });

  it('holds the line when an image grows in an earlier turn', () => {
    const { el, onResize, detachGestures } = readerRestsAt(1320);
    const lineBefore = el.blockRelTop('t2', READ_BLOCK);

    el.setTurnHeightOf('t0', 400 + IMAGE_GROWTH);
    onResize();

    expect(el.scrollTop).toBe(1320 + IMAGE_GROWTH);
    expect(el.blockRelTop('t2', READ_BLOCK)).toBe(lineBefore);
    detachGestures();
  });

  it('keeps an explicit placement made before the resize and its scroll event', () => {
    const { el, onResize, detachGestures } = readerRestsAt(1320);
    markNavigationScroll(el, 400);
    el.setTurnHeightOf('t0', 400 + IMAGE_GROWTH);
    onResize();

    expect(el.scrollTop).toBe(400);
    detachGestures();
  });

  it('keeps Scroll to top at the top when older turns draw after a same-offset placement', () => {
    const viewport = mockContainer({ scrollHeight: 2400 });
    let inserted = false;
    const turn = (offset: () => number) => ({
      getBoundingClientRect: () => ({
        top: viewport.getBoundingClientRect().top + offset() - viewport.scrollTop,
        height: 400,
      }),
    });
    const older = turn(() => 0);
    const first = turn(() => inserted ? 400 : 0);
    const el = Object.assign(viewport, { children: [first] });
    const scroller = el as unknown as HTMLElement;
    const { onResize, detachGestures } = makeScrollObservers(scroller);
    onResize();

    markNavigationScroll(scroller, 0);
    inserted = true;
    el.children.unshift(older);
    onResize();

    expect(el.scrollTop).toBe(0);
    detachGestures();
  });

  it('growth below the line moves nobody', () => {
    const { el, onResize, detachGestures } = readerRestsAt(1320);
    const lineBefore = el.blockRelTop('t2', READ_BLOCK);

    el.setBlockHeight('t2', 3, 900);
    el.setTurnHeightOf('t5', 2000);
    onResize();

    expect(el.scrollTop).toBe(1320);
    expect(el.blockRelTop('t2', READ_BLOCK)).toBe(lineBefore);
    detachGestures();
  });
});
