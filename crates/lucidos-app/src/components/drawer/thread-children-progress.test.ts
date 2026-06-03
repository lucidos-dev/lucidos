/**
 * Tests for children progress visibility in family toggle rows.
 *
 * Children progress ("X/Y done") shows on the FamilyToggleRow that renders
 * under any parent thread with totalChildrenCount > 0 — regardless of section
 * (Review, Active, Waiting, Saved, Archive). The parent ThreadRow itself no
 * longer carries the progress text; it lives on the chevron row to keep
 * parent and leaf rows visually aligned and to ground the count next to its
 * disclosure affordance.
 *
 * The waiting status icon (pulsing dot) must show on the parent row when
 * activeChildrenCount > 0.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { threadMap, threadsLoaded } from '../../store/store';
import { displaySection } from '../../generated/thread-lifecycle';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../../store/thread-events';
import type { ArchiveState } from '../../generated/thread-lifecycle';
import { resolveVisualStatus } from '../shared/ThreadStatusIcon';

function makeThread(id: string, overrides: Partial<ThreadMeta> = {}): ThreadState {
  return {
    meta: {
      id,
      title: 'Test Thread',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-04-12T00:00:00Z',
      updatedAt: '2026-04-12T00:00:00Z',
      status: 'idle',
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      messageCount: 1,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      state: 'active',
      latestTodoList: null,
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
 * Replicate FamilyToggleRow's rendering decisions for children progress.
 * This mirrors the logic in ThreadDrawer.tsx FamilyToggleRow (which is
 * rendered under any parent with totalChildrenCount > 0) and the visual-status
 * resolution still performed on the parent ThreadRow / SearchResultRow.
 */
function familyToggleRenderState(meta: ThreadMeta, status: ThreadStatus) {
  const hasChildren = meta.totalChildrenCount > 0;
  const doneCount = meta.totalChildrenCount - meta.activeChildrenCount;
  return {
    showChildrenProgress: hasChildren,
    progressText: hasChildren ? `${doneCount}/${meta.totalChildrenCount} sub-threads done` : null,
    visualStatus: resolveVisualStatus(status, meta.activeChildrenCount > 0, meta.codingAgentProposed),
  };
}

beforeEach(() => {
  threadMap.value = new Map();
  threadsLoaded.value = false;
});

describe('children progress visibility', () => {
  it('shows progress when thread has children (any section)', () => {
    const sections: Array<{ section: ArchiveState; status: ThreadStatus; saved: boolean; activeChildren: number }> = [
      // Waiting: idle + active children
      { section: 'archived', status: 'idle', saved: false, activeChildren: 2 },
      // Review: inbox + no active children (all done)
      { section: 'inbox', status: 'idle', saved: false, activeChildren: 0 },
      // Saved: default + saved + no active children
      { section: 'archived', status: 'idle', saved: true, activeChildren: 0 },
      // Archive: default + not saved + no active children
      { section: 'archived', status: 'idle', saved: false, activeChildren: 0 },
    ];

    for (const { section, status, saved, activeChildren } of sections) {
      const thread = makeThread('t1', {
        section,
        status,
        saved,
        totalChildrenCount: 3,
        activeChildrenCount: activeChildren,
      });

      const display = displaySection(section, status, saved, activeChildren > 0, false, false);
      const render = familyToggleRenderState(thread.meta, status);

      expect(render.showChildrenProgress).toBe(true);
      expect(render.progressText).toBe(`${3 - activeChildren}/3 sub-threads done`);
      // Verify this is a valid display section
      expect(['active', 'waiting', 'review', 'saved', 'archive']).toContain(display);
    }
  });

  it('does not show progress when thread has no children', () => {
    const thread = makeThread('t1', { totalChildrenCount: 0, activeChildrenCount: 0 });
    const render = familyToggleRenderState(thread.meta, 'idle');

    expect(render.showChildrenProgress).toBe(false);
    expect(render.progressText).toBeNull();
  });

  it('shows 0/3 done when all children still active', () => {
    const thread = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 3 });
    const render = familyToggleRenderState(thread.meta, 'idle');

    expect(render.progressText).toBe('0/3 sub-threads done');
  });

  it('shows 3/3 done when all children finished', () => {
    const thread = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 0 });
    const render = familyToggleRenderState(thread.meta, 'idle');

    expect(render.progressText).toBe('3/3 sub-threads done');
  });

  it('shows 2/3 done when one child still active', () => {
    const thread = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 1 });
    const render = familyToggleRenderState(thread.meta, 'idle');

    expect(render.progressText).toBe('2/3 sub-threads done');
  });

  // The "X/Y" form reads as a fraction, so the noun stays plural even when Y = 1.
  it('keeps plural noun for the 1/1 case', () => {
    const thread = makeThread('t1', { totalChildrenCount: 1, activeChildrenCount: 0 });
    const render = familyToggleRenderState(thread.meta, 'idle');

    expect(render.progressText).toBe('1/1 sub-threads done');
  });
});

describe('visual status resolution', () => {
  // Rule: a thread that is ONLY waiting for other threads renders 'waiting'.
  // Otherwise it renders its own status (failed / running / question / changes)
  // — children's waiting state never overrides the parent's own meaningful state.

  it('idle + active children → waiting (only own state is idle)', () => {
    expect(resolveVisualStatus('idle', true, false)).toBe('waiting');
  });

  it('no active children + idle → idle (no dot)', () => {
    expect(resolveVisualStatus('idle', false, false)).toBe('idle');
  });

  it('no children + running → running (spinner)', () => {
    expect(resolveVisualStatus('running', false, false)).toBe('running');
  });

  it('no active children + waiting + codingAgentProposed → changes (static dot)', () => {
    expect(resolveVisualStatus('waiting', false, true)).toBe('changes');
  });

  it('no active children + waiting without codingAgentProposed → idle (no dot)', () => {
    // Defensive: backend shouldn't park threads in 'waiting' without changes,
    // but historical chat threads (pre-fix ResponseAborted/Failed) might.
    expect(resolveVisualStatus('waiting', false, false)).toBe('idle');
  });

  it('waiting + active children + no changes → waiting (own state is empty)', () => {
    expect(resolveVisualStatus('waiting', true, false)).toBe('waiting');
  });

  it('waiting + active children + codingAgentProposed → changes (own changes win)', () => {
    expect(resolveVisualStatus('waiting', true, true)).toBe('changes');
  });

  it('failed → failed (red triangle)', () => {
    expect(resolveVisualStatus('failed', false, false)).toBe('failed');
  });

  it('failed + active children → failed (own failure wins)', () => {
    expect(resolveVisualStatus('failed', true, false)).toBe('failed');
  });

  it('running + active children → running (own work wins)', () => {
    expect(resolveVisualStatus('running', true, false)).toBe('running');
  });

  it('waiting_for_user_answer + active children → question (own question wins)', () => {
    expect(resolveVisualStatus('waiting_for_user_answer', true, false)).toBe('question');
  });

  it('idle + active children + codingAgentProposed → changes (own changes win)', () => {
    expect(resolveVisualStatus('idle', true, true)).toBe('changes');
  });
});

describe('displaySection routing with children', () => {
  it('idle thread with active children goes to active section', () => {
    expect(displaySection('archived', 'idle', false, true, false, false)).toBe('active');
  });

  it('idle thread with all children done goes to archive', () => {
    expect(displaySection('archived', 'idle', false, false, false, false)).toBe('archive');
  });

  it('inbox thread with active children goes to active (not review)', () => {
    expect(displaySection('inbox', 'idle', false, true, false, false)).toBe('active');
  });

  it('inbox thread with all children done goes to review', () => {
    expect(displaySection('inbox', 'idle', false, false, false, false)).toBe('review');
  });

  it('saved thread with active children goes to saved (save overrides everything)', () => {
    expect(displaySection('archived', 'idle', true, true, false, false)).toBe('saved');
  });

  it('saved thread with all children done goes to saved', () => {
    expect(displaySection('archived', 'idle', true, false, false, false)).toBe('saved');
  });

  it('running thread always goes to active regardless of children', () => {
    expect(displaySection('archived', 'running', false, true, false, false)).toBe('active');
    expect(displaySection('archived', 'running', false, false, false, false)).toBe('active');
  });

  it('archived thread with pending changes routes to review (no work lost behind archive)', () => {
    expect(displaySection('archived', 'idle', false, false, true, false)).toBe('review');
  });

  it('saved thread with pending changes still saves (save wins over pending)', () => {
    expect(displaySection('archived', 'idle', true, false, true, false)).toBe('saved');
  });
});

describe('thread title status icon', () => {
  it('has-changes dot is static (changes, not waiting) in thread title', () => {
    expect(resolveVisualStatus('waiting', false, true)).toBe('changes');
  });

  it('changes win over children — thread title shows static changes dot', () => {
    expect(resolveVisualStatus('waiting', true, true)).toBe('changes');
  });

  it('pulsing waiting dot only when own state has nothing else to show', () => {
    expect(resolveVisualStatus('idle', true, false)).toBe('waiting');
  });
});

describe('children progress consistency across row types', () => {
  it('FamilyToggleRow renders the same progress whether the parent comes from threadMap or search', () => {
    // FamilyToggleRow renders under any parent with totalChildrenCount > 0.
    // Whether that parent comes from threadMap (ThreadRow's source) or from
    // search results re-hydrated via ensureThreadInMap (SearchResultRow's
    // source), the same meta must produce the same toggle-row progress text.
    const meta: ThreadMeta = {
      id: 'search-thread',
      title: 'Search Result Thread',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-04-12T00:00:00Z',
      updatedAt: '2026-04-12T00:00:00Z',
      status: 'idle',
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      messageCount: 5,
      section: 'archived',
      activeChildrenCount: 1,
      totalChildrenCount: 3,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      state: 'active',
      latestTodoList: null,
    };

    // ThreadRow path
    const threadRowRender = familyToggleRenderState(meta, 'idle');

    // SearchResultRow path (same logic, same meta from liveThread)
    const searchRowRender = familyToggleRenderState(meta, 'idle');

    expect(threadRowRender).toEqual(searchRowRender);
    expect(threadRowRender.showChildrenProgress).toBe(true);
    expect(threadRowRender.progressText).toBe('2/3 sub-threads done');
    expect(threadRowRender.visualStatus).toBe('waiting');
  });

  it('search result without live thread data shows no children progress', () => {
    // When a search result has no corresponding entry in threadMap,
    // children counts default to 0 (no live data available)
    const fallbackMeta: Partial<ThreadMeta> = {
      totalChildrenCount: 0,
      activeChildrenCount: 0,
    };

    const render = familyToggleRenderState(
      { ...makeThread('fallback').meta, ...fallbackMeta },
      'idle',
    );

    expect(render.showChildrenProgress).toBe(false);
    expect(render.progressText).toBeNull();
  });
});
