// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import {
  documentReplayHost,
  pickReplayTarget,
  replayEdgeTap,
  type ReplayCandidate,
  type ReplayHost,
} from '../edgeTapReplay';
import MOBILE_SWIPE_CONTAINER from '../MobileSwipeContainer.tsx?raw';
import { TAP_MOVE_THRESHOLD_PX } from '../../../utils/tapGesture';
import { LONG_PRESS_DELAY_MS } from '../../../hooks/useLongPress';

// An edge tap lands on a transparent strip over the row, and the strip's gate
// cancels its touchstart, which kills the synthetic click. replayEdgeTap gives
// the click back to the row, for a tap only. The decisions run on plain
// objects, so this needs no DOM.

const el = (tagName: string, ...classes: string[]): ReplayCandidate => ({
  tagName,
  classList: { contains: (name) => classes.includes(name) },
});

describe('pickReplayTarget', () => {
  it('skips the strip and takes the row beneath it', () => {
    const row = el('DIV', 'thread-row');
    expect(pickReplayTarget([el('DIV', 'edge-swipe-zone'), row, el('DIV', 'pane')])).toBe(row);
  });

  it('takes the topmost element when no strip covers it', () => {
    const top = el('BUTTON');
    expect(pickReplayTarget([top, el('DIV')])).toBe(top);
  });

  it('never lands in an app frame', () => {
    expect(pickReplayTarget([el('DIV', 'edge-swipe-zone'), el('IFRAME'), el('DIV')])).toBeNull();
  });

  it('finds nothing under a stack of strips alone', () => {
    expect(pickReplayTarget([el('DIV', 'edge-swipe-zone'), el('DIV', 'edge-swipe-zone')])).toBeNull();
  });
});

interface FakeTouch {
  identifier: number;
  screenX: number;
  screenY: number;
  clientX: number;
  clientY: number;
}

const touch = (identifier: number, x: number, y = 100): FakeTouch => ({
  identifier, screenX: x, screenY: y, clientX: x, clientY: y,
});

/** A document that records its listeners and what the replay clicks. */
function fakeHost(stack: ReplayCandidate[]) {
  const listeners = new Map<string, Array<(e: never) => void>>();
  const clicks: Array<{ target: ReplayCandidate; x: number; y: number }> = [];
  let clock = 0;
  const host: ReplayHost = {
    addEventListener: (type, fn) => {
      listeners.set(type, [...(listeners.get(type) ?? []), fn]);
    },
    removeEventListener: (type, fn) => {
      listeners.set(type, (listeners.get(type) ?? []).filter((l) => l !== fn));
    },
    elementsFromPoint: () => stack,
    now: () => clock,
    dispatchClick: (target, x, y) => { clicks.push({ target, x, y }); },
  };
  const fire = (type: string, e: { changedTouches: FakeTouch[]; touches: FakeTouch[] }) => {
    for (const fn of listeners.get(type) ?? []) (fn as (e: unknown) => void)(e);
  };
  return {
    host,
    clicks,
    fire,
    advance: (ms: number) => { clock += ms; },
    watching: () => [...listeners.values()].some((l) => l.length > 0),
  };
}

const row = el('DIV', 'thread-row');
const stack = [el('DIV', 'edge-swipe-zone'), row];

function begin(x = 8) {
  const t = fakeHost(stack);
  const start = touch(1, x);
  replayEdgeTap(t.host, { changedTouches: [start], touches: [start] });
  return { ...t, start };
}

describe('replayEdgeTap', () => {
  it('clicks the row under the finger when the touch lifts as a tap', () => {
    const t = begin();
    t.advance(80);
    t.fire('touchend', { changedTouches: [touch(1, 9)], touches: [] });
    expect(t.clicks).toEqual([{ target: row, x: 9, y: 100 }]);
  });

  it('clicks exactly once and stops watching', () => {
    const t = begin();
    t.fire('touchend', { changedTouches: [touch(1, 8)], touches: [] });
    t.fire('touchend', { changedTouches: [touch(1, 8)], touches: [] });
    expect(t.clicks).toHaveLength(1);
    expect(t.watching()).toBe(false);
  });

  it('replays nothing for a drag past the tap threshold', () => {
    const t = begin();
    t.fire('touchmove', { changedTouches: [touch(1, 8 + TAP_MOVE_THRESHOLD_PX + 1)], touches: [touch(1, 20)] });
    t.fire('touchend', { changedTouches: [touch(1, 8)], touches: [] });
    expect(t.clicks).toEqual([]);
  });

  it('replays nothing for a long press', () => {
    const t = begin();
    t.advance(LONG_PRESS_DELAY_MS + 1);
    t.fire('touchend', { changedTouches: [touch(1, 8)], touches: [] });
    expect(t.clicks).toEqual([]);
  });

  it('replays nothing when a second finger joined', () => {
    const t = begin();
    t.fire('touchstart', { changedTouches: [touch(2, 200)], touches: [t.start, touch(2, 200)] });
    t.fire('touchend', { changedTouches: [touch(1, 8)], touches: [touch(2, 200)] });
    expect(t.clicks).toEqual([]);
  });

  it('replays nothing for a cancelled touch, and stops watching', () => {
    const t = begin();
    t.fire('touchcancel', { changedTouches: [touch(1, 8)], touches: [] });
    expect(t.watching()).toBe(false);
    expect(t.clicks).toEqual([]);
  });

  it('ignores the lift of another finger', () => {
    const t = begin();
    t.fire('touchend', { changedTouches: [touch(2, 200)], touches: [t.start] });
    expect(t.clicks).toEqual([]);
    expect(t.watching()).toBe(true);
  });

  it('replays nothing when the stack holds only strips or a frame', () => {
    const t = fakeHost([el('DIV', 'edge-swipe-zone'), el('IFRAME')]);
    const start = touch(1, 8);
    replayEdgeTap(t.host, { changedTouches: [start], touches: [start] });
    t.fire('touchend', { changedTouches: [touch(1, 8)], touches: [] });
    expect(t.clicks).toEqual([]);
  });

  it('measures travel in screen space, not client space', () => {
    const t = begin();
    // The visual viewport shifted under a still finger: client moved, screen did not.
    const lift = { ...touch(1, 8), clientX: 8 + 3 * TAP_MOVE_THRESHOLD_PX };
    t.fire('touchend', { changedTouches: [lift], touches: [] });
    expect(t.clicks).toHaveLength(1);
  });

  it('the returned stop ends the watch', () => {
    const t = fakeHost(stack);
    const start = touch(1, 8);
    const stop = replayEdgeTap(t.host, { changedTouches: [start], touches: [start] });
    stop();
    expect(t.watching()).toBe(false);
  });
});

describe('the strip gate replays what it cancels', () => {
  const gate = MOBILE_SWIPE_CONTAINER.match(/function gateStripTouch[\s\S]*?\n\}\n/)?.[0] ?? '';

  it('replays only a touch whose preventDefault took', () => {
    expect(gate).toMatch(
      /e\.preventDefault\(\);\s*(\/\/[^\n]*\n\s*)*if \(e\.defaultPrevented\) replayEdgeTap\(documentReplayHost\(\), e\);\s*\}/,
    );
  });
});

describe('documentReplayHost', () => {
  it('focuses a text field before it clicks it', () => {
    const input = document.createElement('input');
    document.body.appendChild(input);
    let focusedAtClick = false;
    input.addEventListener('click', () => { focusedAtClick = document.activeElement === input; });
    documentReplayHost().dispatchClick(input, 1, 1);
    expect(focusedAtClick).toBe(true);
    input.remove();
  });

  it('leaves focus alone for a row', () => {
    const row = document.createElement('div');
    row.tabIndex = 0;
    document.body.appendChild(row);
    let clicked = false;
    row.addEventListener('click', () => { clicked = true; });
    documentReplayHost().dispatchClick(row, 1, 1);
    expect(clicked).toBe(true);
    expect(document.activeElement).not.toBe(row);
    row.remove();
  });
});
