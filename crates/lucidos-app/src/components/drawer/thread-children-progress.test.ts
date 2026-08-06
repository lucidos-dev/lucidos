/**
 * Tests for the sub-thread disclosure control on family parent rows — a single
 * toggle anchored at the parent row's bottom-center that swaps contents by
 * collapse state:
 *
 *   - EXPANDED  → the ▴ chevron alone (the affordance to collapse back).
 *   - COLLAPSED → the sub-thread count badge alone (no chevron); the badge
 *                 itself signals there are hidden sub-threads, and clicking it
 *                 re-expands.
 *
 * The control only renders in the nested ThreadList (collapsible context);
 * search / drafts render flat lists, so they show neither chevron nor badge.
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

/**
 * Replicate ThreadRowContentImpl's family-disclosure decisions. Mirrors the
 * logic in ThreadDrawer.tsx — the disclosure control renders when `collapsible
 * && totalChildrenCount > 0`, and swaps contents by collapse state: the ▴
 * chevron ONLY while expanded, the count badge (totalChildrenCount) ONLY while
 * collapsed (the two are mutually exclusive). Also surfaces the visual-status
 * resolution still performed on every parent row (ThreadRow / SearchResultRow).
 */
function familyRenderState(
  meta: ThreadMeta,
  status: ThreadStatus,
  opts: { collapsible: boolean; isCollapsed: boolean } = { collapsible: true, isCollapsed: false },
) {
  const hasFamily = opts.collapsible && meta.totalChildrenCount > 0;
  const hasActiveChildren = meta.activeChildrenCount > 0;
  const a11yCount = `${meta.totalChildrenCount} sub-thread${meta.totalChildrenCount === 1 ? '' : 's'}`;
  return {
    // The toggle button itself renders whenever the family is collapsible.
    hasDisclosure: hasFamily,
    // Chevron only when EXPANDED; badge only when COLLAPSED — never both.
    showChevron: hasFamily && !opts.isCollapsed,
    showCount: hasFamily && opts.isCollapsed,
    countText: hasFamily && opts.isCollapsed ? String(meta.totalChildrenCount) : null,
    a11yCount,
    // The control's own tooltip + aria-label, so hovering the badge/chevron
    // never falls through to the row's general thread tooltip.
    disclosureLabel: opts.isCollapsed ? `Show ${a11yCount}` : 'Hide sub-threads',
    visualStatus: resolveVisualStatus(status, hasActiveChildren, meta.codingAgentProposed),
  };
}

beforeEach(() => {
  threadMap.value = new Map();
  threadsLoaded.value = false;
});

describe('family disclosure visibility', () => {
  it('shows the chevron (no badge) when an EXPANDED collapsible thread has children (any section)', () => {
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
      // Default opts = expanded: chevron shown, count badge absent.
      const render = familyRenderState(thread.meta, status);

      expect(render.hasDisclosure).toBe(true);
      expect(render.showChevron).toBe(true);
      expect(render.showCount).toBe(false);
      // Verify this is a valid display section
      expect(['current', 'saved', 'archive']).toContain(display);
    }
  });

  it('does not show the disclosure control when the thread has no children', () => {
    const thread = makeThread('t1', { totalChildrenCount: 0, activeChildrenCount: 0 });
    const render = familyRenderState(thread.meta, 'idle');

    expect(render.hasDisclosure).toBe(false);
    expect(render.showChevron).toBe(false);
    expect(render.showCount).toBe(false);
  });

  it('does not show the disclosure control in a non-collapsible context (search / drafts)', () => {
    const thread = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 1 });
    const render = familyRenderState(thread.meta, 'idle', { collapsible: false, isCollapsed: false });

    expect(render.hasDisclosure).toBe(false);
    expect(render.showChevron).toBe(false);
    expect(render.showCount).toBe(false);
  });

  it('collapsed = badge only (no chevron); expanded = chevron only (no badge)', () => {
    const thread = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 1 });
    const expanded = familyRenderState(thread.meta, 'idle', { collapsible: true, isCollapsed: false });
    const collapsed = familyRenderState(thread.meta, 'idle', { collapsible: true, isCollapsed: true });

    // The toggle button is present in both states — only its contents swap.
    expect(expanded.hasDisclosure).toBe(true);
    expect(collapsed.hasDisclosure).toBe(true);

    // Expanded: chevron, no badge.
    expect(expanded.showChevron).toBe(true);
    expect(expanded.showCount).toBe(false);
    expect(expanded.countText).toBeNull();

    // Collapsed: badge, no chevron.
    expect(collapsed.showChevron).toBe(false);
    expect(collapsed.showCount).toBe(true);
    expect(collapsed.countText).toBe('3');
  });

  it('badge shows the total sub-thread count regardless of how many are done', () => {
    const opts = { collapsible: true, isCollapsed: true };
    const allActive = makeThread('t1', { totalChildrenCount: 3, activeChildrenCount: 3 });
    const allDone = makeThread('t2', { totalChildrenCount: 3, activeChildrenCount: 0 });
    const one = makeThread('t3', { totalChildrenCount: 1, activeChildrenCount: 0 });

    expect(familyRenderState(allActive.meta, 'idle', opts).countText).toBe('3');
    expect(familyRenderState(allDone.meta, 'idle', opts).countText).toBe('3');
    expect(familyRenderState(one.meta, 'idle', opts).countText).toBe('1');
  });

  it('keeps the aria label smart-plural (1 sub-thread, N sub-threads)', () => {
    const one = makeThread('t1', { totalChildrenCount: 1, activeChildrenCount: 0 });
    const many = makeThread('t2', { totalChildrenCount: 3, activeChildrenCount: 0 });

    expect(familyRenderState(one.meta, 'idle').a11yCount).toBe('1 sub-thread');
    expect(familyRenderState(many.meta, 'idle').a11yCount).toBe('3 sub-threads');
  });

  it('disclosure label is "Show N sub-threads" collapsed, "Hide sub-threads" expanded', () => {
    const one = makeThread('t1', { totalChildrenCount: 1, activeChildrenCount: 0 });
    const many = makeThread('t2', { totalChildrenCount: 3, activeChildrenCount: 0 });
    const collapsed = { collapsible: true, isCollapsed: true };
    const expanded = { collapsible: true, isCollapsed: false };

    // Collapsed names the hidden count (smart-plural).
    expect(familyRenderState(one.meta, 'idle', collapsed).disclosureLabel).toBe('Show 1 sub-thread');
    expect(familyRenderState(many.meta, 'idle', collapsed).disclosureLabel).toBe('Show 3 sub-threads');
    // Expanded drops the count — children are listed inline.
    expect(familyRenderState(many.meta, 'idle', expanded).disclosureLabel).toBe('Hide sub-threads');
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
    expect(resolveVisualStatus('waiting', false, true)).toBe('changes');
  });

  it('changes win over children — thread title shows static changes dot', () => {
    expect(resolveVisualStatus('waiting', true, true)).toBe('changes');
  });

  it('pulsing waiting dot only when own state has nothing else to show', () => {
    expect(resolveVisualStatus('idle', true, false)).toBe('waiting');
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
    // Expanded nested row: control present, showing the chevron.
    expect(threadRow.hasDisclosure).toBe(true);
    expect(threadRow.showChevron).toBe(true);
    // Flat search row: no control at all.
    expect(searchRow.hasDisclosure).toBe(false);
    expect(searchRow.showChevron).toBe(false);
  });

  it('a parent with no children shows no disclosure control in either context', () => {
    const meta = { ...makeThread('fallback').meta, totalChildrenCount: 0, activeChildrenCount: 0 };

    expect(familyRenderState(meta, 'idle', { collapsible: true, isCollapsed: true }).hasDisclosure).toBe(false);
    expect(familyRenderState(meta, 'idle', { collapsible: false, isCollapsed: false }).hasDisclosure).toBe(false);
  });
});
