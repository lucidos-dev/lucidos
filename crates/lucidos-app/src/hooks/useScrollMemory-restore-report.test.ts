import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../utils/clientLog', () => ({ postClientLog: vi.fn() }));

import { attachScrollMemory } from './useScrollMemory';
import { postClientLog } from '../utils/clientLog';
import { setFollowLiveEdge } from '../components/chat/scrollState';
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
afterEach(() => { vi.useRealTimers(); setFollowLiveEdge(false); });

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

  it('the content pane and the drawer report nothing', () => {
    const detach = attachScrollMemory(mockTranscript({ ids: IDS, turnHeight: 1000 }), 'k', { live: () => ({}) });
    detach();
    expect(log).not.toHaveBeenCalled();
  });
});
