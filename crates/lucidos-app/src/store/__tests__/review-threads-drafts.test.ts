/**
 * `getReviewThreads()` powers `attentionThreadCount`, the badge shown in the
 * thread toggle and mobile header. The badge must match the visible Review
 * section in the drawer — which means draft threads must be excluded here
 * too, with the same focused-on-desktop carve-out used by ThreadDrawer's
 * categorizeThreads.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import {
  threadMap,
  drafts,
  focusedThreadId,
  getReviewThreads,
  attentionThreadCount,
  type DraftMeta,
} from '../store';
import type { ThreadState, ThreadMeta } from '../thread-events';

const STUB_META: DraftMeta = { title: 'New thread', updatedAt: '' };

function makeUnreadThread(id: string, overrides: Partial<ThreadMeta> = {}): ThreadState {
  return {
    meta: {
      id,
      title: 'Test Thread',
      channel: 'chat',
      initiator: 'user',
      pinned: false,
      createdAt: '2026-04-12T00:00:00Z',
      updatedAt: '2026-04-12T00:00:00Z',
      unread: true,
      status: 'idle',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      messageCount: 1,
      section: 'unread',
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

beforeEach(() => {
  threadMap.value = new Map();
  drafts.value = new Map();
  focusedThreadId.value = null;
  (globalThis as any).innerWidth = 1024;
});

describe('getReviewThreads — draft exclusion', () => {
  it('excludes unread thread that has an unsent draft', () => {
    const t = makeUnreadThread('t1');
    threadMap.value = new Map([['t1', t]]);
    drafts.value = new Map([['t1', STUB_META]]);

    expect(getReviewThreads()).toHaveLength(0);
    expect(attentionThreadCount.value).toBe(0);
  });

  it('includes unread thread without a draft', () => {
    const t = makeUnreadThread('t1');
    threadMap.value = new Map([['t1', t]]);

    expect(getReviewThreads().map(t => t.meta.id)).toEqual(['t1']);
    expect(attentionThreadCount.value).toBe(1);
  });

  it('still includes a draft thread when it is focused on desktop', () => {
    // Matches ThreadDrawer's carve-out: focused-on-desktop drafts stay in
    // their natural section so live updates remain visible.
    const t = makeUnreadThread('t1');
    threadMap.value = new Map([['t1', t]]);
    drafts.value = new Map([['t1', STUB_META]]);
    focusedThreadId.value = 't1';

    expect(getReviewThreads()).toHaveLength(1);
  });

  it('excludes the focused draft on mobile (no desktop-style focus)', () => {
    (globalThis as any).innerWidth = 375;
    const t = makeUnreadThread('t1');
    threadMap.value = new Map([['t1', t]]);
    drafts.value = new Map([['t1', STUB_META]]);
    focusedThreadId.value = 't1';

    expect(getReviewThreads()).toHaveLength(0);
  });

  it('counts only non-draft review threads in a mixed set', () => {
    threadMap.value = new Map([
      ['draft', makeUnreadThread('draft')],
      ['review1', makeUnreadThread('review1')],
      ['review2', makeUnreadThread('review2')],
    ]);
    drafts.value = new Map([['draft', STUB_META]]);

    expect(getReviewThreads().map(t => t.meta.id).sort()).toEqual(['review1', 'review2']);
    expect(attentionThreadCount.value).toBe(2);
  });
});
