import type { ComponentChildren } from 'preact';
import { beforeEach, describe, expect, it } from 'vitest';
import {
  ThreadToggleButton, threadListVisible, threadToggleBadgeCount,
} from '../ThreadToggleButton';
import { MOBILE_PANE_CONFIGS } from '../../layout/MobileAppHeader';
import { HamburgerButton } from '../../layout/ContentNav';
import { componentTypes, findByClass, textOf, type AnyVNode } from '../../layout/__tests__/vnodeWalk';
import { mobileView, threadDrawerOpen, threadMap } from '../../../store/store';
import { viewportIsMobile } from '../../../utils/viewport';
import type { ThreadState, ThreadStatus } from '../../../store/thread-events';

function makeThread(id: string, status: ThreadStatus): ThreadState {
  return {
    meta: {
      id,
      title: id,
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-05-01T00:00:00Z',
      updatedAt: '2026-05-01T00:00:00Z',
      status,
      summaryVersion: 0,
      messageCount: 1,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function seedBlocked(n: number): void {
  const threads: ThreadState[] = [];
  for (let i = 0; i < n; i++) threads.push(makeThread(`t${i}`, 'waiting_for_user_answer'));
  threadMap.value = new Map(threads.map(t => [t.meta.id, t]));
}

function renderToggle(): AnyVNode {
  return ThreadToggleButton({}) as AnyVNode;
}

function badges(): AnyVNode[] {
  return findByClass(renderToggle(), 'badge');
}

beforeEach(() => {
  threadMap.value = new Map();
  threadDrawerOpen.value = false;
  mobileView.value = 'thread';
  viewportIsMobile.value = false;
});

describe('threadListVisible', () => {
  it('follows the drawer flag on desktop and the visible pane on mobile', () => {
    expect(threadListVisible(false, 'thread', true)).toBe(true);
    expect(threadListVisible(false, 'thread', false)).toBe(false);
    expect(threadListVisible(true, 'threads', false)).toBe(true);
    expect(threadListVisible(true, 'thread', true)).toBe(false);
    expect(threadListVisible(true, 'content', true)).toBe(false);
  });
});

describe('threadToggleBadgeCount', () => {
  it('passes the count through only while the thread list is hidden', () => {
    expect(threadToggleBadgeCount(3, false, 'thread', false)).toBe(3);
    expect(threadToggleBadgeCount(3, false, 'thread', true)).toBe(0);
    expect(threadToggleBadgeCount(3, true, 'thread', false)).toBe(3);
    expect(threadToggleBadgeCount(3, true, 'threads', false)).toBe(0);
  });

  it('never badges when nothing is blocked', () => {
    expect(threadToggleBadgeCount(0, false, 'thread', false)).toBe(0);
    expect(threadToggleBadgeCount(0, true, 'thread', false)).toBe(0);
  });
});

describe('ThreadToggleButton badge', () => {
  it('renders the count when the desktop drawer is hidden', () => {
    seedBlocked(2);
    const found = badges();
    expect(found).toHaveLength(1);
    expect(textOf(found[0])).toBe('2');
  });

  it('drops the badge when the desktop drawer is showing the list', () => {
    seedBlocked(2);
    threadDrawerOpen.value = true;
    expect(badges()).toHaveLength(0);
  });

  it('renders the count on the mobile thread pane', () => {
    viewportIsMobile.value = true;
    seedBlocked(1);
    expect(textOf(badges()[0])).toBe('1');
  });

  it('drops the badge on the mobile threads pane, where the list is on screen', () => {
    viewportIsMobile.value = true;
    mobileView.value = 'threads';
    seedBlocked(1);
    expect(badges()).toHaveLength(0);
  });

  it('renders no badge when nothing is blocked', () => {
    expect(badges()).toHaveLength(0);
  });

  it('puts the count in the accessible label, not only in decorative markup', () => {
    seedBlocked(4);
    expect(String(renderToggle().props['aria-label'])).toContain('4');
  });
});

function rowChildTypes(header: AnyVNode): unknown[] {
  const row = findByClass(header, 'mobile-header-row')[0];
  const kids = row.props.children as ComponentChildren;
  const list = Array.isArray(kids) ? kids : [kids];
  return list
    .filter(k => k !== null && k !== undefined && typeof k === 'object')
    .map(k => (k as AnyVNode).type);
}

describe('mobile thread pane header', () => {
  it('brackets the row: thread-drawer toggle leading, menu hamburger trailing', () => {
    viewportIsMobile.value = true;
    const types = rowChildTypes((MOBILE_PANE_CONFIGS.thread.Header as () => AnyVNode)());
    expect(types[0]).toBe(ThreadToggleButton);
    expect(types[types.length - 1]).toBe(HamburgerButton);
  });

  it('keeps the menu-drawer hamburger on the content pane header', () => {
    viewportIsMobile.value = true;
    const types = componentTypes((MOBILE_PANE_CONFIGS.content.Header as () => AnyVNode)());
    expect(types).toContain(HamburgerButton);
  });
});
