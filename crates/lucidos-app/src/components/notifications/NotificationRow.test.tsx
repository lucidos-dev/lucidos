// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import type { Tap } from '@lucidos/sdk';

vi.mock('../shared/Skeleton', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../shared/Skeleton')>()),
  useSkeleton: () => false,
}));

vi.mock('../../store/actions/in-app-notification-toast', () => ({
  dispatchDeepLink: vi.fn(),
}));

vi.mock('../../store/actions/notifications', () => ({
  viewNotification: vi.fn(() => Promise.resolve()),
  markAllRead: vi.fn(),
  loadMoreNotifications: vi.fn(),
  setNotificationsFilter: vi.fn(),
}));

import { NotificationRow } from './NotificationsView';
import { dispatchDeepLink } from '../../store/actions/in-app-notification-toast';
import { viewNotification } from '../../store/actions/notifications';
import type { Notification } from '../../store/types';

interface AnyVNode extends VNode<{ children?: ComponentChildren; class?: string; [k: string]: unknown }> {}

function findByClass(node: ComponentChildren, cls: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByClass(c, cls);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  const klass = (v.props?.class as string | undefined) ?? '';
  if (typeof klass === 'string' && klass.split(/\s+/).includes(cls)) return v;
  return findByClass(v.props?.children, cls);
}

function classesOf(node: AnyVNode | null): string[] {
  return ((node?.props?.class as string | undefined) ?? '').split(/\s+/).filter(Boolean);
}

function click(node: AnyVNode | null): void {
  const onClick = node?.props?.onClick as (() => void) | undefined;
  if (!onClick) throw new Error('node has no onClick');
  onClick();
}

const NAVIGATE_TAP: Tap = {
  kind: 'navigate',
  to: { target: 'thread', id: 't-1', event_id: 'e-1' },
};

function notification(over: Partial<Notification> = {}): Notification {
  return {
    id: 'n-1',
    title: 'Question waiting',
    message: 'The agent needs an answer',
    created_at: '2026-08-22T10:00:00Z',
    read: false,
    ...over,
  };
}

describe('NotificationRow', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('dispatches the row deep link, carrying its tap and source event', () => {
    const n = notification({ thread_id: 't-1', event_id: 'e-1', tap: NAVIGATE_TAP });
    click(findByClass(NotificationRow({ n }), 'notification-item'));
    expect(dispatchDeepLink).toHaveBeenCalledWith({
      notification: 'n-1',
      thread: 't-1',
      event: 'e-1',
      tap: NAVIGATE_TAP,
    });
  });

  it('dispatches for a modal row too, which resolves to the detail as before', () => {
    const n = notification({ tap: { kind: 'modal' } });
    click(findByClass(NotificationRow({ n }), 'notification-item'));
    expect(dispatchDeepLink).toHaveBeenCalledWith({
      notification: 'n-1',
      thread: null,
      event: null,
      tap: { kind: 'modal' },
    });
  });

  it('gives a navigate row a chevron, on the shared row-icon tap target', () => {
    const tree = NotificationRow({ n: notification({ tap: NAVIGATE_TAP }) });
    const chevron = findByClass(tree, 'notification-row-detail-btn');
    expect(chevron).not.toBeNull();
    expect(classesOf(chevron)).toEqual(
      expect.arrayContaining(['icon-btn', 'row-icon']),
    );
  });

  it.each<[string, Tap | undefined]>([
    ['a modal tap', { kind: 'modal' }],
    ['no tap at all', undefined],
  ])('gives %s no chevron', (_label, tap) => {
    const tree = NotificationRow({ n: notification({ tap }) });
    expect(findByClass(tree, 'notification-row-detail-btn')).toBeNull();
  });

  it('the chevron opens the notification detail and navigates nowhere', () => {
    const tree = NotificationRow({ n: notification({ tap: NAVIGATE_TAP }) });
    click(findByClass(tree, 'notification-row-detail-btn'));
    expect(viewNotification).toHaveBeenCalledWith('n-1');
    expect(dispatchDeepLink).not.toHaveBeenCalled();
  });

  it('keeps the chevron OUT of the row button', () => {
    const tree = NotificationRow({ n: notification({ tap: NAVIGATE_TAP }) });
    const item = findByClass(tree, 'notification-item');
    expect(item!.type).toBe('button');
    expect(findByClass(item!.props.children, 'notification-row-detail-btn')).toBeNull();
  });

  it('keeps .notification-item a real button, which the e2e click needs', () => {
    const tree = NotificationRow({ n: notification() });
    expect(findByClass(tree, 'notification-item')!.type).toBe('button');
  });

  it('marks an unread row on the wrapper, whose surface spans the chevron', () => {
    const unread = NotificationRow({ n: notification({ read: false, tap: NAVIGATE_TAP }) });
    expect(classesOf(findByClass(unread, 'notification-row'))).toContain('unread');
    const read = NotificationRow({ n: notification({ read: true }) });
    expect(classesOf(findByClass(read, 'notification-row'))).not.toContain('unread');
  });
});
