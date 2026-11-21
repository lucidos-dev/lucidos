import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../utils/clientLog', () => ({ postClientLog: vi.fn() }));

import { attachScrollMemory } from './useScrollMemory';
import { postClientLog } from '../utils/clientLog';
import { makeScrollObservers, setFollowLiveEdge, readerGestureForTest } from '../components/chat/scrollState';
import { mockTranscript } from '../components/chat/__tests__/scroll-test-helpers';

/** Each open of the transcript says once which rule placed the reader. A
 *  report like "it opened at the bottom" then names its route in engine.log.
 *  Plan: docs/plans/2026-09-23-the-reading-position-and-the-thumb-hold-still.md */

const log = postClientLog as unknown as ReturnType<typeof vi.fn>;
const IDS = ['t0', 't1', 't2'];
const transcript = { live: () => ({}), resetOnEmpty: true, anchorsToContent: true };

beforeEach(() => {
  localStorage.clear();
  log.mockClear();
  setFollowLiveEdge(false);
  class Inert { observe() {} disconnect() {} takeRecords() { return []; } }
  (globalThis as any).ResizeObserver = Inert;
  (globalThis as any).MutationObserver = Inert;
});
afterEach(() => { readerGestureForTest(null, false); vi.useRealTimers(); vi.restoreAllMocks(); setFollowLiveEdge(false); });

const outcomes = () => log.mock.calls.map(([category, message, data]) => [category, message, data]);

describe('the restore reports its outcome', () => {
  it('a thread with no record opens at the top, and says so', () => {
    const detach = attachScrollMemory(mockTranscript({ ids: IDS, turnHeight: 1000, rowsPerTurn: 40 }), 'k', transcript);
    detach();
    expect(outcomes()).toEqual([['scroll', 'restore', { outcome: 'top', form: 'none' }]]);
  });

  it('a row it can reach lands, and says so once', () => {
    localStorage.setItem('k', 'row:0:t0-r32');
    const el = mockTranscript({ ids: IDS, turnHeight: 1000, rowsPerTurn: 40 });
    const detach = attachScrollMemory(el, 'k', transcript);
    el.scrollTop = 1200;
    el.fireScroll();
    detach();
    expect(outcomes()).toEqual([['scroll', 'restore', { outcome: 'landed', form: 'row' }]]);
  });

  it('a row in the last screen lands at the bottom, and names that apart', () => {
    localStorage.setItem('k', 'row:0:t2-r39');
    const detach = attachScrollMemory(mockTranscript({ ids: IDS, turnHeight: 1000, rowsPerTurn: 40 }), 'k', transcript);
    detach();
    expect(outcomes()).toEqual([['scroll', 'restore', { outcome: 'landed-at-bottom', form: 'row' }]]);
  });

  it('a row that never draws gives up, and says so', () => {
    vi.useFakeTimers();
    localStorage.setItem('k', 'row:0:gone-r1');
    const detach = attachScrollMemory(mockTranscript({ ids: IDS, turnHeight: 1000, rowsPerTurn: 40 }), 'k', transcript);
    vi.advanceTimersByTime(3500);
    detach();
    expect(outcomes()).toEqual([['scroll', 'restore', { outcome: 'gave-up', form: 'row' }]]);
  });

  it.each([0, 3500, 4100, 10_000, 20_000])('corrects growth at %d ms before reporting settled drift', delay => {
    // An image above the reader's line, inside the anchored turn, decodes once
    // they are back. The line must not move.
    let now = 0;
    vi.spyOn(performance, 'now').mockImplementation(() => now);
    localStorage.setItem('k', 'anchor:-520:t2');
    const el = mockTranscript({ ids: ['t0', 't1', 't2', 't3'], turnHeight: 400, blocks: { t2: [20, 100, 1000, 200] } });
    const { onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', transcript);

    const restoredTop = el.scrollTop;
    now = delay;
    el.setBlockHeight('t2', 0, 300);
    onResize();
    expect(el.scrollTop).toBe(restoredTop + 280);
    now = 60_000;                         // long past the hold's ceiling
    onResize();
    detach();

    expect(outcomes()).toContainEqual(['scroll', 'restore', { outcome: 'settled', form: 'anchor', drift: 0 }]);
  });

  it.each([20_001, 25_000, 60_000])('releases without writing on a first growth round after the ceiling: %d ms', delay => {
    let now = 0;
    vi.spyOn(performance, 'now').mockImplementation(() => now);
    localStorage.setItem('k', 'anchor:-520:t2');
    const el = mockTranscript({ ids: ['t0', 't1', 't2', 't3'], turnHeight: 400, blocks: { t2: [20, 100, 1000, 200] } });
    const { onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', transcript);
    const restoredTop = el.scrollTop;

    now = delay;
    el.setBlockHeight('t2', 0, 300);
    onResize();
    expect(el.scrollTop).toBe(restoredTop);
    expect(outcomes()).toContainEqual(['scroll', 'restore', { outcome: 'settled', form: 'anchor', drift: 280 }]);
    detach();
  });

  it('releases after the final ceiling correction, even if content keeps growing', () => {
    let now = 0;
    vi.spyOn(performance, 'now').mockImplementation(() => now);
    localStorage.setItem('k', 'anchor:-520:t2');
    const el = mockTranscript({ ids: ['t0', 't1', 't2', 't3'], turnHeight: 400, blocks: { t2: [20, 100, 1000, 200] } });
    const { onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', transcript);
    const restoredTop = el.scrollTop;

    now = 20_000;
    el.setBlockHeight('t2', 0, 300);
    onResize();
    expect(el.scrollTop).toBe(restoredTop + 280);
    expect(outcomes()).toContainEqual(['scroll', 'restore', { outcome: 'settled', form: 'anchor', drift: 0 }]);

    now += 100;
    el.setBlockHeight('t2', 0, 600);
    onResize();
    expect(el.scrollTop).toBe(restoredTop + 280);
    expect(outcomes().filter(([, , data]) => data.outcome === 'settled')).toHaveLength(1);
    detach();
  });

  it('does not correct late growth once the reader takes over', () => {
    let now = 0;
    vi.spyOn(performance, 'now').mockImplementation(() => now);
    localStorage.setItem('k', 'anchor:-520:t2');
    const el = mockTranscript({ ids: ['t0', 't1', 't2', 't3'], turnHeight: 400, blocks: { t2: [20, 100, 1000, 200] } });
    const { onScroll, onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', transcript);

    now = 4100;
    readerGestureForTest(el);
    el.scrollTop = 1000;
    onScroll();
    el.setBlockHeight('t2', 0, 300);
    onResize();
    expect(el.scrollTop).toBe(1000);
    expect(outcomes().some(([, , data]) => data.outcome === 'settled')).toBe(false);
    detach();
  });

  it('the content pane and the drawer report nothing', () => {
    const detach = attachScrollMemory(mockTranscript({ ids: IDS, turnHeight: 1000 }), 'k', { live: () => ({}) });
    detach();
    expect(log).not.toHaveBeenCalled();
  });
});
