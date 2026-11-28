import { describe, it, expect, beforeEach, afterEach } from 'vitest';

if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(0); return 0; };
  (globalThis as any).cancelAnimationFrame = () => {};
}

import { UI_SCALE_MAX, UI_SCALE_MIN, UI_SCALE_STEP } from '@lucidos/appearance';
import { mockContainer } from './scroll-test-helpers';
import {
  awayFromBottom,
  makeScrollObservers,
  notAtTop,
  setActiveScrollElement,
  stepThreadTurn,
  stopFollowingBottom,
} from '../scrollState';
import { osReducesMotion } from '../../../utils/motion';


const SCALES: number[] = [];
for (let scale = UI_SCALE_MIN; scale <= UI_SCALE_MAX; scale += UI_SCALE_STEP) SCALES.push(scale);

const ROOT_PX_AT_100 = 16;
const rem = (scale: number) => (ROOT_PX_AT_100 * scale) / 100;
const lineOf = (scale: number) => rem(scale) * 0.75 * 1.5;
const SUBPIXEL = 0.5;

const RESTING_SLACK_PX = 2;
const TOP_WINDOW_PX = 80;
const TURN_NAV_SLACK_PX = 4;

function transcriptOf(lines: number, scale: number, viewportLines = 20) {
  return mockContainer({
    scrollHeight: lines * lineOf(scale),
    clientHeight: viewportLines * lineOf(scale),
  });
}

function readerScrollsTo(el: ReturnType<typeof mockContainer>, top: number) {
  el.scrollTop = top;
  const { onScroll, detachGestures } = makeScrollObservers(el as unknown as HTMLElement);
  onScroll();
  detachGestures();
}

describe('the px scroll thresholds at every UI scale', () => {
  beforeEach(() => {
    stopFollowingBottom();
    awayFromBottom.value = false;
    notAtTop.value = false;
  });
  afterEach(() => {
    setActiveScrollElement(null);
  });

  it('covers the whole range of the setting', () => {
    expect(SCALES[0]).toBe(UI_SCALE_MIN);
    expect(SCALES[SCALES.length - 1]).toBe(UI_SCALE_MAX);
  });

  describe.each(SCALES)('the resting slack at %d%%', (scale) => {
    const edgeOf = (el: ReturnType<typeof mockContainer>) => el.scrollHeight - el.clientHeight;

    it('reads a rest inside the slack as the live edge', () => {
      const el = transcriptOf(200, scale);
      readerScrollsTo(el, edgeOf(el) - RESTING_SLACK_PX);
      expect(awayFromBottom.value).toBe(false);
    });

    it('reads a rest just past the slack as away from the edge', () => {
      const el = transcriptOf(200, scale);
      readerScrollsTo(el, edgeOf(el) - RESTING_SLACK_PX - SUBPIXEL);
      expect(awayFromBottom.value).toBe(true);
    });

    it('reads one line short of the edge as away from it', () => {
      expect(lineOf(scale)).toBeGreaterThan(RESTING_SLACK_PX + SUBPIXEL);
      const el = transcriptOf(200, scale);
      readerScrollsTo(el, edgeOf(el) - lineOf(scale));
      expect(awayFromBottom.value).toBe(true);
    });
  });

  describe.each(SCALES)('the top window at %d%%', (scale) => {
    it('ends at the same px offset', () => {
      const el = transcriptOf(200, scale);
      readerScrollsTo(el, TOP_WINDOW_PX);
      expect(notAtTop.value).toBe(false);
      readerScrollsTo(el, TOP_WINDOW_PX + SUBPIXEL);
      expect(notAtTop.value).toBe(true);
    });

    it('spans more than a line and less than half a short viewport', () => {
      const lines = TOP_WINDOW_PX / lineOf(scale);
      expect(lines).toBeGreaterThan(2);
      expect(lines).toBeLessThan(6);
      const shortViewportPx = 480;
      expect(TOP_WINDOW_PX).toBeLessThan(shortViewportPx / 2);
    });
  });

  describe.each(SCALES)('the turn-step slack at %d%%', (scale) => {
    let restoreGCS: () => void;
    const gap = rem(scale) * 0.5;

    beforeEach(() => {
      osReducesMotion.value = true;
      const origGCS = (globalThis as any).getComputedStyle;
      (globalThis as any).getComputedStyle = () => ({ scrollMarginTop: `${gap}px`, display: 'block' });
      restoreGCS = () => { (globalThis as any).getComputedStyle = origGCS; };
    });
    afterEach(() => {
      osReducesMotion.value = false;
      restoreGCS();
    });

    function oneLineTurns(count: number) {
      const tops = Array.from({ length: count }, (_, i) => i * lineOf(scale));
      const el: any = {
        scrollTop: 0,
        scrollHeight: (count + 40) * lineOf(scale),
        clientHeight: 20 * lineOf(scale),
        parentElement: null,
        focus() {},
        getBoundingClientRect: () => ({ top: 0, width: 400, height: el.clientHeight }),
        querySelectorAll: () => turns,
      };
      const turns = tops.map((top) => ({
        parentElement: null,
        classList: { add() {}, remove() {}, contains: () => false },
        getBoundingClientRect: () => ({ top: top - el.scrollTop, width: 400, height: lineOf(scale) }),
      }));
      return { el, tops };
    }

    it.each([-SUBPIXEL, 0, SUBPIXEL])('steps to the next one-line turn from a rest %d px off the line', (drift) => {
      expect(lineOf(scale)).toBeGreaterThan(2 * TURN_NAV_SLACK_PX);
      const { el, tops } = oneLineTurns(30);
      setActiveScrollElement(el);
      const landedOn = 10;
      el.scrollTop = tops[landedOn] - gap + drift;

      stepThreadTurn(1);

      expect(el.scrollTop).toBeCloseTo(tops[landedOn + 1] - gap, 5);
    });

    it('treats a turn as landed only inside the slack', () => {
      const { el, tops } = oneLineTurns(30);
      setActiveScrollElement(el);
      const landedOn = 10;
      el.scrollTop = tops[landedOn] - gap - (TURN_NAV_SLACK_PX - SUBPIXEL);
      stepThreadTurn(1);
      expect(el.scrollTop).toBeCloseTo(tops[landedOn + 1] - gap, 5);

      el.scrollTop = tops[landedOn] - gap - (TURN_NAV_SLACK_PX + SUBPIXEL);
      stepThreadTurn(1);
      expect(el.scrollTop).toBeCloseTo(tops[landedOn] - gap, 5);
    });

    it.each([-SUBPIXEL, 0, SUBPIXEL])('steps to the previous one-line turn from a rest %d px off the line', (drift) => {
      const { el, tops } = oneLineTurns(30);
      setActiveScrollElement(el);
      const landedOn = 10;
      el.scrollTop = tops[landedOn] - gap + drift;

      stepThreadTurn(-1);

      expect(el.scrollTop).toBeCloseTo(tops[landedOn - 1] - gap, 5);
    });
  });
});
