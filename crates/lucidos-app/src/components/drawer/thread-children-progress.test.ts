/**
 * Tests for the sub-thread toggle on family parent rows: a "Show / Hide N
 * sub-threads" link under the date. Both states name the count.
 *
 * The control only renders in the nested ThreadList (collapsible context);
 * search / drafts render flat lists, so they show no toggle.
 *
 * The waiting status icon (pulsing dot) must show on the parent row when
 * activeChildrenCount > 0.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { threadMap, threadsLoaded } from '../../store/store';
import { displaySection } from '../../generated/thread-lifecycle';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../../store/thread-events';
import type { ArchiveState } from '../../generated/thread-lifecycle';
import { resolveVisualStatus, visualStatusFor } from '../shared/ThreadStatusIcon';
import { familyDisclosureLabel } from './ThreadDrawer';

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
      liveEventWaitCount: 0,
      liveEventWaits: [],
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

/** ThreadRowContentImpl renders the toggle when `collapsible &&
 *  totalChildrenCount > 0`. Also surfaces the visual-status resolution still
 *  performed on every parent row (ThreadRow / SearchResultRow). */
function familyRenderState(
  meta: ThreadMeta,
  status: ThreadStatus,
  opts: { collapsible: boolean; isCollapsed: boolean } = { collapsible: true, isCollapsed: false },
) {
  const hasDisclosure = opts.collapsible && meta.totalChildrenCount > 0;
  return {
    hasDisclosure,
    label: hasDisclosure ? familyDisclosureLabel(meta.totalChildrenCount, opts.isCollapsed) : null,
    visualStatus: visualStatusFor(status, meta),
  };
}

beforeEach(() => {
  threadMap.value = new Map();
  threadsLoaded.value = false;
});

describe('family disclosure visibility', () => {
  it('shows the toggle when a collapsible thread has children (any section)', () => {
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
      expect(familyRenderState(thread.meta, status).hasDisclosure).toBe(true);
      expect(['current', 'saved', 'archive']).toContain(display);
    }
  });

  it('does not show the toggle when the thread has no children', () => {
    const thread = makeThread('t1', { totalChildrenCount: 0, activeChildrenCount: 0 });
    expect(familyRenderState(thread.meta, 'idle').hasDisclosure).toBe(false);
  });

  it('does not show the toggle in a non-collapsible context (search / drafts)', () => {
    const thread = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 1 });
    const render = familyRenderState(thread.meta, 'idle', { collapsible: false, isCollapsed: false });
    expect(render.hasDisclosure).toBe(false);
  });

  it('names the total count in both states, whatever is done', () => {
    expect(familyDisclosureLabel(3, true)).toBe('Show 3 sub-threads');
    expect(familyDisclosureLabel(3, false)).toBe('Hide 3 sub-threads');
    const allDone = makeThread('t2', { totalChildrenCount: 3, activeChildrenCount: 0 });
    expect(familyRenderState(allDone.meta, 'idle', { collapsible: true, isCollapsed: true }).label)
      .toBe('Show 3 sub-threads');
  });

  it('is smart-plural (1 sub-thread, N sub-threads)', () => {
    expect(familyDisclosureLabel(1, true)).toBe('Show 1 sub-thread');
    expect(familyDisclosureLabel(1, false)).toBe('Hide 1 sub-thread');
    expect(familyDisclosureLabel(12, true)).toBe('Show 12 sub-threads');
  });
});

describe('visual status resolution', () => {
  // Rule: a thread that is ONLY waiting for other threads renders 'waiting'.
  // Otherwise it renders its own status (failed / running / question / changes)
  // — children's waiting state never overrides the parent's own meaningful state.

  it('idle + active children → waiting (only own state is idle)', () => {
    expect(resolveVisualStatus('idle', true, false, false)).toBe('waiting');
  });

  it('no active children + idle → idle (no dot)', () => {
    expect(resolveVisualStatus('idle', false, false, false)).toBe('idle');
  });

  it('no children + running → running (spinner)', () => {
    expect(resolveVisualStatus('running', false, false, false)).toBe('running');
  });

  it('no active children + waiting + codingAgentProposed → changes (static dot)', () => {
    expect(resolveVisualStatus('waiting', false, true, false)).toBe('changes');
  });

  it('no active children + waiting without codingAgentProposed → idle (no dot)', () => {
    // Defensive: backend shouldn't park threads in 'waiting' without changes,
    // but historical chat threads (pre-fix ResponseAborted/Failed) might.
    expect(resolveVisualStatus('waiting', false, false, false)).toBe('idle');
  });

  it('waiting + active children + no changes → waiting (own state is empty)', () => {
    expect(resolveVisualStatus('waiting', true, false, false)).toBe('waiting');
  });

  // Changes outrank active children: the child writes its own worktree, so
  // the parent's change is whole and Apply is offered to match (ADR 0249).
  it('waiting + active children + codingAgentProposed → changes (resolvable)', () => {
    expect(resolveVisualStatus('waiting', true, true, false)).toBe('changes');
  });

  it('failed → failed (red triangle)', () => {
    expect(resolveVisualStatus('failed', false, false, false)).toBe('failed');
  });

  it('failed + active children → failed (own failure wins)', () => {
    expect(resolveVisualStatus('failed', true, false, false)).toBe('failed');
  });

  it('running + active children → running (own work wins)', () => {
    expect(resolveVisualStatus('running', true, false, false)).toBe('running');
  });

  it('waiting_for_user_answer + active children → question (own question wins)', () => {
    expect(resolveVisualStatus('waiting_for_user_answer', true, false, false)).toBe('question');
  });

  it('idle + active children + codingAgentProposed → changes (resolvable)', () => {
    expect(resolveVisualStatus('idle', true, true, false)).toBe('changes');
  });
});

describe('displaySection routing with children', () => {
  it('idle thread with active children goes to current section', () => {
    expect(displaySection('archived', 'idle', false, true, false, false)).toBe('current');
  });

  it('idle thread with all children done goes to archive', () => {
    expect(displaySection('archived', 'idle', false, false, false, false)).toBe('archive');
  });

  it('inbox thread with active children stays in current', () => {
    expect(displaySection('inbox', 'idle', false, true, false, false)).toBe('current');
  });

  it('inbox thread with all children done goes to current', () => {
    expect(displaySection('inbox', 'idle', false, false, false, false)).toBe('current');
  });

  it('saved thread with active children goes to saved (save overrides everything)', () => {
    expect(displaySection('archived', 'idle', true, true, false, false)).toBe('saved');
  });

  it('saved thread with all children done goes to saved', () => {
    expect(displaySection('archived', 'idle', true, false, false, false)).toBe('saved');
  });

  it('running thread always goes to current regardless of children', () => {
    expect(displaySection('archived', 'running', false, true, false, false)).toBe('current');
    expect(displaySection('archived', 'running', false, false, false, false)).toBe('current');
  });

  it('archived thread with pending changes routes to current (no work lost behind archive)', () => {
    expect(displaySection('archived', 'idle', false, false, true, false)).toBe('current');
  });

  it('saved thread with pending changes still saves (save wins over pending)', () => {
    expect(displaySection('archived', 'idle', true, false, true, false)).toBe('saved');
  });
});

describe('thread title status icon', () => {
  it('has-changes dot is static (changes, not waiting) in thread title', () => {
    expect(resolveVisualStatus('waiting', false, true, false)).toBe('changes');
  });

  // A running child does not pulse over it: the parent's change is whole and
  // Apply is offered, so the title reads changes (ADR 0249).
  it('changes outrank children, so the thread title stays static', () => {
    expect(resolveVisualStatus('waiting', true, true, false)).toBe('changes');
  });

  it('pulsing waiting dot only when own state has nothing else to show', () => {
    expect(resolveVisualStatus('idle', true, false, false)).toBe('waiting');
  });
});

describe('family disclosure consistency across row types', () => {
  it('resolves the same parent status from threadMap or search, but only the nested list shows the control', () => {
    // Whether the parent comes from threadMap (ThreadRow's source) or from a
    // search result re-hydrated via ensureThreadInMap (SearchResultRow's
    // source), the waiting/active dot resolves identically. The disclosure
    // control is intentionally NOT shown in flat search results — only the
    // nested ThreadList passes `collapsible`.
    const meta: ThreadMeta = {
      ...makeThread('search-thread').meta,
      messageCount: 5,
      activeChildrenCount: 1,
      totalChildrenCount: 3,
    };

    const threadRow = familyRenderState(meta, 'idle', { collapsible: true, isCollapsed: false });
    const searchRow = familyRenderState(meta, 'idle', { collapsible: false, isCollapsed: false });

    expect(threadRow.visualStatus).toBe('waiting');
    expect(searchRow.visualStatus).toBe(threadRow.visualStatus);
    // Nested row: toggle present. Flat search row: no toggle at all.
    expect(threadRow.hasDisclosure).toBe(true);
    expect(searchRow.hasDisclosure).toBe(false);
  });

  it('a parent with no children shows no disclosure control in either context', () => {
    const meta = { ...makeThread('fallback').meta, totalChildrenCount: 0, activeChildrenCount: 0 };

    expect(familyRenderState(meta, 'idle', { collapsible: true, isCollapsed: true }).hasDisclosure).toBe(false);
    expect(familyRenderState(meta, 'idle', { collapsible: false, isCollapsed: false }).hasDisclosure).toBe(false);
  });
});
