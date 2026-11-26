import { describe, it, expect, beforeEach } from 'vitest';
import type { VNode } from 'preact';
import { MobileThreadTitleBar } from '../MobileAppHeader';
import { threadMap, focusedThreadId } from '../../../store/store';
import type { ThreadState, ThreadMeta } from '../../../store/thread-events';

const THREAD_ID = 'pinned-1';

function makeThread(): ThreadState {
  const meta: ThreadMeta = {
    id: THREAD_ID,
    title: 'Pinned title',
    channel: 'chat',
    initiator: 'user',
    saved: false,
    createdAt: '2026-05-01T00:00:00Z',
    updatedAt: '2026-05-01T00:00:00Z',
    status: 'idle',
    summaryVersion: 0,
    messageCount: 1,
    section: 'inbox',
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0,
    attentionDescendantCount: 0,
    archivableSubThreadCount: 0,
    codingAgentChangeState: { kind: 'none' },
    codingAgentIsExternalRepo: false,
    lastRevivedAt: '',
    state: 'active',
    latestTodoList: null,
    liveEventWaitCount: 0,
    liveEventWaits: [],
  };
  return {
    meta,
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

describe('the mobile thread title row is the scroller-pinned row', () => {
  beforeEach(() => {
    threadMap.value = new Map([[THREAD_ID, makeThread()]]);
    focusedThreadId.value = THREAD_ID;
  });

  it('carries the marker the iOS repaint nudge reads to skip it', () => {
    const row = MobileThreadTitleBar() as VNode<Record<string, unknown>>;
    expect(row.props['data-scroller-pinned']).toBe(true);
    expect(String(row.props.class)).toContain('mobile-thread-title-row');
  });
});
