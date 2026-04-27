/**
 * Drafts must appear ONLY in the Drafts section of the drawer — never
 * duplicated into Review, History, Pinned, Running, or Waiting.
 *
 * Exception: a draft thread that is currently focused on desktop appears in
 * its natural section so the user can still see live updates while typing.
 * (On mobile there is no desktop-style "focus" — the draft always wins.)
 */

import { describe, it, expect } from 'vitest';
import { categorizeThreads } from './ThreadDrawer';
import type { ThreadState, ThreadMeta } from '../../store/thread-events';
import type { StoredSection } from '../../generated/thread-lifecycle';
import type { DraftMeta } from '../../store/store';

const STUB_META: DraftMeta = { title: 'New thread', updatedAt: '' };

function draftMap(...ids: string[]): Map<string, DraftMeta> {
  return new Map(ids.map(id => [id, STUB_META]));
}

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

function withSection(id: string, section: StoredSection, extra: Partial<ThreadMeta> = {}): ThreadState {
  return makeThread(id, { section, ...extra });
}

describe('categorizeThreads — drafts excluded from other sections', () => {
  it('draft on an unread thread goes to drafts only, not review', () => {
    const t = withSection('t1', 'unread');
    const result = categorizeThreads([t], draftMap('t1'), null, false);

    expect(result.drafts).toHaveLength(1);
    expect(result.drafts[0].meta.id).toBe('t1');
    expect(result.review).toHaveLength(0);
    expect(result.history).toHaveLength(0);
    expect(result.pinned).toHaveLength(0);
    expect(result.running).toHaveLength(0);
    expect(result.waiting).toHaveLength(0);
  });

  it('draft on a default thread goes to drafts only, not history', () => {
    const t = withSection('t1', 'default');
    const result = categorizeThreads([t], draftMap('t1'), null, false);

    expect(result.drafts).toHaveLength(1);
    expect(result.history).toHaveLength(0);
  });

  it('draft on a pinned thread goes to drafts only, not pinned', () => {
    const t = withSection('t1', 'default', { pinned: true });
    const result = categorizeThreads([t], draftMap('t1'), null, false);

    expect(result.drafts).toHaveLength(1);
    expect(result.pinned).toHaveLength(0);
  });

  it('non-draft unread thread still goes to review (regression check)', () => {
    const t = withSection('t1', 'unread');
    const result = categorizeThreads([t], draftMap(), null, false);

    expect(result.drafts).toHaveLength(0);
    expect(result.review).toHaveLength(1);
    expect(result.review[0].meta.id).toBe('t1');
  });

  it('mixes drafts and non-drafts into the right sections', () => {
    const draft1 = withSection('draft1', 'unread');
    const review1 = withSection('review1', 'unread');
    const history1 = withSection('history1', 'default');

    const result = categorizeThreads(
      [draft1, review1, history1],
      draftMap('draft1'),
      null,
      false,
    );

    expect(result.drafts.map(t => t.meta.id)).toEqual(['draft1']);
    expect(result.review.map(t => t.meta.id)).toEqual(['review1']);
    expect(result.history.map(t => t.meta.id)).toEqual(['history1']);
  });

  it('focused draft on desktop still appears in its natural section', () => {
    // Desktop convention: when the user is looking at a draft thread, the
    // drawer shows it in its natural section so live updates stay visible.
    const t = withSection('t1', 'unread');
    const result = categorizeThreads([t], draftMap('t1'), 't1', false);

    expect(result.drafts).toHaveLength(0);
    expect(result.review).toHaveLength(1);
  });

  it('focused draft on mobile goes to drafts (no desktop-style focus)', () => {
    const t = withSection('t1', 'unread');
    const result = categorizeThreads([t], draftMap('t1'), 't1', true);

    expect(result.drafts).toHaveLength(1);
    expect(result.review).toHaveLength(0);
  });

  it('statusMap is populated for all threads, including drafts', () => {
    const draft = withSection('draft', 'unread');
    const other = withSection('other', 'default');
    const result = categorizeThreads([draft, other], draftMap('draft'), null, false);

    expect(result.statusMap.get('draft')).toBe('idle');
    expect(result.statusMap.get('other')).toBe('idle');
  });
});
