import { describe, it, expect, beforeEach, afterEach } from 'vitest';

if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(0); return 0; };
  (globalThis as any).cancelAnimationFrame = () => {};
}

import { mockTranscript } from './scroll-test-helpers';
import {
  clearPendingEventScroll,
  makeScrollObservers,
  readerGestureForTest,
  setFollowLiveEdge,
} from '../scrollState';

// An unarmed reader sits mid-thread with no arrival hold: they scrolled there
// themselves. Content above their line then grows with no change of width.
// The cases marked TODAY record that the line is pushed down the viewport.
// They are a temporary measure, and phase 4 of the plan flips them:
// docs/temporary-measures.md § "TODAY tests pin a late image pushing the line".

const IDS = ['t0', 't1', 't2', 't3', 't4', 't5'];
const T2_BLOCKS = [20, 100, 1000, 200];
const READ_BLOCK = 2;
const IMAGE_BLOCK = 0;
const IMAGE_GROWTH = 280;

describe('a late image above an unarmed reader, outside any hold', () => {
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

  it('TODAY: an image above the line, in the same turn, pushes the line down', () => {
    const { el, onResize, detachGestures } = readerRestsAt(1320);
    const lineBefore = el.blockRelTop('t2', READ_BLOCK);

    el.setBlockHeight('t2', IMAGE_BLOCK, T2_BLOCKS[IMAGE_BLOCK] + IMAGE_GROWTH);
    onResize();

    expect(el.scrollTop).toBe(1320);
    expect(el.blockRelTop('t2', READ_BLOCK) - lineBefore).toBe(IMAGE_GROWTH);
    detachGestures();
  });

  it('TODAY: an image in an earlier turn pushes the line down', () => {
    const { el, onResize, detachGestures } = readerRestsAt(1320);
    const lineBefore = el.blockRelTop('t2', READ_BLOCK);

    el.setTurnHeightOf('t0', 400 + IMAGE_GROWTH);
    onResize();

    expect(el.scrollTop).toBe(1320);
    expect(el.blockRelTop('t2', READ_BLOCK) - lineBefore).toBe(IMAGE_GROWTH);
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
