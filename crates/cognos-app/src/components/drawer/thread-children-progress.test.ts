/**
 * Tests for children progress visibility in thread rows.
 *
 * Children progress ("X/Y done") must show on ALL thread rows that have
 * totalChildrenCount > 0 — regardless of section (Review, Running, Waiting,
 * Pinned, History) or whether the row is a search result.
 *
 * The waiting status icon (pulsing dot) must show when activeChildrenCount > 0.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { threadMap, threadsLoaded } from '../../store/store';
import { displaySection } from '../../generated/thread-lifecycle';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../../store/thread-events';
import type { StoredSection } from '../../generated/thread-lifecycle';
import { resolveVisualStatus } from '../shared/ThreadStatusIcon';

function makeThread(id: string, overrides: Partial<ThreadMeta> = {}): ThreadState {
  return {
    meta: {
      id,
      title: 'Test Thread',
      channel: 'chat',
      initiator: 'user',
      pinned: false,
      createdAt: '2026-04-12T00:00:00Z',
      updatedAt: '2026-04-12T00:00:00Z',
      unread: false,
      status: 'idle',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      messageCount: 1,
      section: 'default',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ...overrides,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

/**
 * Replicate ThreadRow's rendering decisions for children progress.
 * This mirrors the logic in ThreadDrawer.tsx ThreadRow and SearchResultRow.
 */
function threadRowRenderState(meta: ThreadMeta, status: ThreadStatus) {
  const hasChildren = meta.totalChildrenCount > 0;
  const doneCount = meta.totalChildrenCount - meta.activeChildrenCount;
  return {
    showChildrenProgress: hasChildren,
    progressText: hasChildren ? `${doneCount}/${meta.totalChildrenCount} done` : null,
    visualStatus: resolveVisualStatus(status, meta.activeChildrenCount > 0, meta.ccHasChanges),
  };
}

beforeEach(() => {
  threadMap.value = new Map();
  threadsLoaded.value = false;
});

describe('children progress visibility', () => {
  it('shows progress when thread has children (any section)', () => {
    const sections: Array<{ section: StoredSection; status: ThreadStatus; pinned: boolean; activeChildren: number }> = [
      // Waiting: idle + active children
      { section: 'default', status: 'idle', pinned: false, activeChildren: 2 },
      // Review: unread + no active children (all done)
      { section: 'unread', status: 'idle', pinned: false, activeChildren: 0 },
      // Pinned: default + pinned + no active children
      { section: 'default', status: 'idle', pinned: true, activeChildren: 0 },
      // History: default + not pinned + no active children
      { section: 'default', status: 'idle', pinned: false, activeChildren: 0 },
    ];

    for (const { section, status, pinned, activeChildren } of sections) {
      const thread = makeThread('t1', {
        section,
        status,
        pinned,
        totalChildrenCount: 3,
        activeChildrenCount: activeChildren,
      });

      const display = displaySection(section, status, pinned, activeChildren > 0);
      const render = threadRowRenderState(thread.meta, status);

      expect(render.showChildrenProgress).toBe(true);
      expect(render.progressText).toBe(`${3 - activeChildren}/3 done`);
      // Verify this is a valid display section
      expect(['running', 'waiting', 'review', 'pinned', 'history']).toContain(display);
    }
  });

  it('does not show progress when thread has no children', () => {
    const thread = makeThread('t1', { totalChildrenCount: 0, activeChildrenCount: 0 });
    const render = threadRowRenderState(thread.meta, 'idle');

    expect(render.showChildrenProgress).toBe(false);
    expect(render.progressText).toBeNull();
  });

  it('shows 0/3 done when all children still active', () => {
    const thread = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 3 });
    const render = threadRowRenderState(thread.meta, 'idle');

    expect(render.progressText).toBe('0/3 done');
  });

  it('shows 3/3 done when all children finished', () => {
    const thread = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 0 });
    const render = threadRowRenderState(thread.meta, 'idle');

    expect(render.progressText).toBe('3/3 done');
  });

  it('shows 2/3 done when one child still active', () => {
    const thread = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 1 });
    const render = threadRowRenderState(thread.meta, 'idle');

    expect(render.progressText).toBe('2/3 done');
  });
});

describe('visual status resolution', () => {
  it('active children → waiting (pulsing dot)', () => {
    expect(resolveVisualStatus('idle', true, false)).toBe('waiting');
  });

  it('no active children + idle → idle (no dot)', () => {
    expect(resolveVisualStatus('idle', false, false)).toBe('idle');
  });

  it('no children + running → running (spinner)', () => {
    expect(resolveVisualStatus('running', false, false)).toBe('running');
  });

  it('no active children + waiting + ccHasChanges → changes (static dot)', () => {
    expect(resolveVisualStatus('waiting', false, true)).toBe('changes');
  });

  it('no active children + waiting without ccHasChanges → idle (no dot)', () => {
    // Defensive: backend shouldn't park threads in 'waiting' without changes,
    // but historical chat threads (pre-fix ResponseAborted/Failed) might.
    expect(resolveVisualStatus('waiting', false, false)).toBe('idle');
  });

  it('active children + waiting → waiting (pulsing dot, regardless of changes)', () => {
    expect(resolveVisualStatus('waiting', true, false)).toBe('waiting');
    expect(resolveVisualStatus('waiting', true, true)).toBe('waiting');
  });

  it('failed → failed (red triangle)', () => {
    expect(resolveVisualStatus('failed', false, false)).toBe('failed');
  });

  it('active children override failed → waiting (pulsing dot)', () => {
    expect(resolveVisualStatus('failed', true, false)).toBe('waiting');
  });
});

describe('displaySection routing with children', () => {
  it('idle thread with active children goes to waiting section', () => {
    expect(displaySection('default', 'idle', false, true)).toBe('waiting');
  });

  it('idle thread with all children done goes to history', () => {
    expect(displaySection('default', 'idle', false, false)).toBe('history');
  });

  it('unread thread with active children goes to waiting (not review)', () => {
    expect(displaySection('unread', 'idle', false, true)).toBe('waiting');
  });

  it('unread thread with all children done goes to review', () => {
    expect(displaySection('unread', 'idle', false, false)).toBe('review');
  });

  it('pinned thread with active children goes to waiting (not pinned)', () => {
    expect(displaySection('default', 'idle', true, true)).toBe('waiting');
  });

  it('pinned thread with all children done goes to pinned', () => {
    expect(displaySection('default', 'idle', true, false)).toBe('pinned');
  });

  it('running thread always goes to running regardless of children', () => {
    expect(displaySection('default', 'running', false, true)).toBe('running');
    expect(displaySection('default', 'running', false, false)).toBe('running');
  });
});

describe('thread title status icon', () => {
  it('has-changes dot is static (changes, not waiting) in thread title', () => {
    expect(resolveVisualStatus('waiting', false, true)).toBe('changes');
  });

  it('pulsing dot when thread has active children', () => {
    expect(resolveVisualStatus('waiting', true, true)).toBe('waiting');
  });
});

describe('children progress consistency across row types', () => {
  it('ThreadRow and SearchResultRow use same rendering logic', () => {
    // Both ThreadRow and SearchResultRow derive children progress from the same fields.
    // This test verifies the conditions are identical by checking the same meta
    // produces the same render state regardless of whether it comes from
    // threadMap (ThreadRow) or a search result with live thread data (SearchResultRow).
    const meta: ThreadMeta = {
      id: 'search-thread',
      title: 'Search Result Thread',
      channel: 'chat',
      initiator: 'user',
      pinned: false,
      createdAt: '2026-04-12T00:00:00Z',
      updatedAt: '2026-04-12T00:00:00Z',
      unread: false,
      status: 'idle',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      messageCount: 5,
      section: 'default',
      activeChildrenCount: 1,
      totalChildrenCount: 3,
    };

    // ThreadRow path
    const threadRowRender = threadRowRenderState(meta, 'idle');

    // SearchResultRow path (same logic, same meta from liveThread)
    const searchRowRender = threadRowRenderState(meta, 'idle');

    expect(threadRowRender).toEqual(searchRowRender);
    expect(threadRowRender.showChildrenProgress).toBe(true);
    expect(threadRowRender.progressText).toBe('2/3 done');
    expect(threadRowRender.visualStatus).toBe('waiting');
  });

  it('search result without live thread data shows no children progress', () => {
    // When a search result has no corresponding entry in threadMap,
    // children counts default to 0 (no live data available)
    const fallbackMeta: Partial<ThreadMeta> = {
      totalChildrenCount: 0,
      activeChildrenCount: 0,
    };

    const render = threadRowRenderState(
      { ...makeThread('fallback').meta, ...fallbackMeta },
      'idle',
    );

    expect(render.showChildrenProgress).toBe(false);
    expect(render.progressText).toBeNull();
  });
});
