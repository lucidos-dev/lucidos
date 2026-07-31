/**
 * The thread-drawer toggle carries the needs-attention badge whenever the thread
 * list itself is off screen, on BOTH layouts:
 *
 *   - Desktop: the drawer is closed, so the threads header's Filter button (the
 *     count's other home) is hidden with it.
 *   - Mobile: the thread list is a separate swipe pane, so the count is
 *     unreachable from the conversation without it.
 *
 * The complementary half matters just as much: with the list visible the toggle
 * must NOT badge, or the same number is claimed twice side by side.
 *
 * These tests invoke the components directly and walk the returned VNode tree
 * without descending into nested function components (ThreadNav, ControlPanel,
 * ...), which use render-time hooks. Both components under test are hook-free at
 * their own level, so direct invocation is safe. The badge's semantics
 * (attention-only, excluding review / running) live in
 * `components/drawer/attention-view.test.ts`.
 */
import type { ComponentChildren, VNode } from 'preact';
import { beforeEach, describe, expect, it } from 'vitest';
import {
  ThreadToggleButton, threadListVisible, threadToggleBadgeCount,
} from '../ThreadToggleButton';
import { MOBILE_PANE_CONFIGS } from '../../layout/MobileAppHeader';
import { HamburgerButton } from '../../layout/PanelNav';
import { mobileView, threadDrawerOpen, threadMap } from '../../../store/store';
import { viewportIsMobile } from '../../../utils/viewport';
import type { ThreadState, ThreadStatus } from '../../../store/thread-events';

type AnyVNode = VNode<Record<string, unknown>>;

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
      messageCount: 1,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentHasDiff: false,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

/** Seed `threadMap` with `n` threads that each need the user's attention. */
function seedAttention(n: number): void {
  const threads: ThreadState[] = [];
  for (let i = 0; i < n; i++) threads.push(makeThread(`t${i}`, 'waiting_for_user_answer'));
  threadMap.value = new Map(threads.map(t => [t.meta.id, t]));
}

/** Collect DOM (string-typed) vnodes matching `cls`. Deliberately does NOT
 *  descend into function components, so no nested hooks are ever invoked. */
function findByClass(node: ComponentChildren, cls: string): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(n => findByClass(n, cls));
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const out: AnyVNode[] = [];
  const klass = (v.props.class as string | undefined) ?? '';
  if (klass.split(' ').includes(cls)) out.push(v);
  return out.concat(findByClass(v.props.children as ComponentChildren, cls));
}

/** Every function-component type used anywhere in a vnode subtree. Walks
 *  through DOM nodes and arrays, and records component types without calling
 *  them. */
function componentTypes(node: ComponentChildren): unknown[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(componentTypes);
  const v = node as AnyVNode;
  const here: unknown[] = typeof v.type === 'string' ? [] : [v.type];
  return here.concat(componentTypes(v.props.children as ComponentChildren));
}

function textOf(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return '';
  return textOf(v.props.children as ComponentChildren);
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
    // Mobile ignores the drawer flag entirely: the list is a swipe pane there.
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

  it('never badges when nothing needs attention', () => {
    expect(threadToggleBadgeCount(0, false, 'thread', false)).toBe(0);
    expect(threadToggleBadgeCount(0, true, 'thread', false)).toBe(0);
  });
});

describe('ThreadToggleButton badge', () => {
  it('renders the count when the desktop drawer is hidden', () => {
    seedAttention(2);
    const found = badges();
    expect(found).toHaveLength(1);
    expect(textOf(found[0])).toBe('2');
  });

  it('drops the badge when the desktop drawer is showing the list', () => {
    seedAttention(2);
    threadDrawerOpen.value = true;
    expect(badges()).toHaveLength(0);
  });

  it('renders the count on the mobile thread pane', () => {
    viewportIsMobile.value = true;
    seedAttention(1);
    expect(textOf(badges()[0])).toBe('1');
  });

  it('drops the badge on the mobile threads pane, where the list is on screen', () => {
    viewportIsMobile.value = true;
    mobileView.value = 'threads';
    seedAttention(1);
    expect(badges()).toHaveLength(0);
  });

  it('renders no badge when nothing needs attention', () => {
    expect(badges()).toHaveLength(0);
  });

  it('puts the count in the accessible label, not only in decorative markup', () => {
    seedAttention(4);
    expect(String(renderToggle().props['aria-label'])).toContain('4');
  });
});

/** Types of the header row's DIRECT children, in row order. */
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
