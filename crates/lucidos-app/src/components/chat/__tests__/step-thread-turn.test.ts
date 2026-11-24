import { describe, it, expect, beforeEach, afterEach } from 'vitest';

if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(0); return 0; };
  (globalThis as any).cancelAnimationFrame = () => {};
}

import {
  stepThreadTurn,
  makeScrollObservers,
  setActiveScrollElement,
  awayFromBottom,
  followingLiveEdge,
  setFollowLiveEdge,
  stopFollowingBottom,
} from '../scrollState';
import { osReducesMotion } from '../../../utils/motion';


function makeTurn(contentTop: number, el: { scrollTop: number }, height = 60) {
  return {
    parentElement: null,
    classList: { add() {}, remove() {} },
    getBoundingClientRect: () => ({ top: contentTop - el.scrollTop, width: 400, height }),
  };
}

function makeContainer(opts: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
  const turns: any[] = [];
  const el: any = {
    scrollTop: opts.scrollTop,
    scrollHeight: opts.scrollHeight,
    clientHeight: opts.clientHeight,
    parentElement: null,
    focus() {},
    getBoundingClientRect: () => ({ top: 0, width: 400, height: opts.clientHeight }),
    querySelectorAll: (_sel: string) => turns,
  };
  return { el, turns };
}

describe('stepThreadTurn — down chevron on the last turn', () => {
  let restoreMotion: () => void;
  let restoreGCS: () => void;

  beforeEach(() => {
    awayFromBottom.value = false;
    osReducesMotion.value = true;
    restoreMotion = () => { osReducesMotion.value = false; };
    const origGCS = (globalThis as any).getComputedStyle;
    (globalThis as any).getComputedStyle = () => ({ scrollMarginTop: '8px', display: 'block' });
    restoreGCS = () => { (globalThis as any).getComputedStyle = origGCS; };
  });

  afterEach(() => {
    setActiveScrollElement(null);
    restoreMotion();
    restoreGCS();
  });

  it('does NOT show the chevron when a down-jump lands at the bottom (the regression)', () => {
    const { el, turns } = makeContainer({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(300, el), makeTurn(920, el));
    setActiveScrollElement(el);

    stepThreadTurn(1);

    expect(awayFromBottom.value).toBe(false);
  });

  it('keeps the chevron when a down-jump lands mid-thread (content still below)', () => {
    const { el, turns } = makeContainer({ scrollTop: 0, scrollHeight: 2000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(800, el), makeTurn(1600, el));
    setActiveScrollElement(el);

    stepThreadTurn(1);

    expect(awayFromBottom.value).toBe(true);
  });

  it('returns the transcript it focused, so the caller can move the focused-pane marker', () => {
    const { el, turns } = makeContainer({ scrollTop: 0, scrollHeight: 2000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(800, el));
    setActiveScrollElement(el);

    expect(stepThreadTurn(1)).toBe(el);
  });

  it('reaching the last turn (landed one turn above) lands at the bottom with no chevron', () => {
    const { el, turns } = makeContainer({ scrollTop: 700, scrollHeight: 2000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(700, el), makeTurn(1520, el));
    setActiveScrollElement(el);

    stepThreadTurn(1);

    expect(awayFromBottom.value).toBe(false);
  });

  it('rests the turn on the landing line, clear of the chrome above it', () => {
    const styles = () => ({ scrollMarginTop: '24px', display: 'block' });
    (globalThis as any).getComputedStyle = styles;
    const { el, turns } = makeContainer({ scrollTop: 0, scrollHeight: 4000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(800, el), makeTurn(1600, el));
    setActiveScrollElement(el);

    stepThreadTurn(1);

    expect(el.scrollTop).toBe(776);
  });
});

describe('stepThreadTurn and the standing follow', () => {
  let restoreMotion: () => void;
  let restoreGCS: () => void;

  beforeEach(() => {
    awayFromBottom.value = false;
    stopFollowingBottom();
    osReducesMotion.value = true;
    restoreMotion = () => { osReducesMotion.value = false; };
    const origGCS = (globalThis as any).getComputedStyle;
    (globalThis as any).getComputedStyle = () => ({ scrollMarginTop: '8px', display: 'block' });
    restoreGCS = () => { (globalThis as any).getComputedStyle = origGCS; };
  });

  afterEach(() => {
    stopFollowingBottom();
    setActiveScrollElement(null);
    restoreMotion();
    restoreGCS();
  });

  function ridingLongThread() {
    const { el, turns } = makeContainer({ scrollTop: 0, scrollHeight: 2000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(800, el), makeTurn(1600, el));
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    expect(followingLiveEdge.value).toBe(true);
    return el;
  }

  it('ends the ride on a step that parks the reader mid-thread', () => {
    ridingLongThread();

    stepThreadTurn(-1);

    expect(awayFromBottom.value).toBe(true);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('keeps the ride on a step that lands at the live edge', () => {
    const { el, turns } = makeContainer({ scrollTop: 700, scrollHeight: 2000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(700, el), makeTurn(1520, el));
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    expect(followingLiveEdge.value).toBe(true);

    stepThreadTurn(1);

    expect(awayFromBottom.value).toBe(false);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('LEAVES the reader on the turn once the step\'s own scroll event lands', () => {
    const el = ridingLongThread();
    const { onScroll } = makeScrollObservers(el);
    onScroll();

    stepThreadTurn(-1);
    const landed = el.scrollTop;
    expect(landed).toBeLessThan(1500);
    onScroll();

    expect(el.scrollTop).toBe(landed);
    expect(followingLiveEdge.value).toBe(false);
  });
});
