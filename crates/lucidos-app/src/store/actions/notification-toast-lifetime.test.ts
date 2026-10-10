import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Notification } from '../types';

// The toast module reaches the navigation dispatcher through `resolveDeepLink`,
// so the whole navigate graph loads with it. None of it runs here: these keep
// the import inert, exactly as `__tests__/notification-toast-requested.test.ts`
// does for the arrival matrix.
vi.mock('./threads', () => ({
  focusThread: vi.fn(),
  unfocusThread: vi.fn(),
  focusThreadOrBootstrap: vi.fn(),
}));
vi.mock('./apps', () => ({
  loadApps: vi.fn(),
  openAppById: vi.fn(),
  refreshAppUI: vi.fn(),
  captureAppUI: vi.fn(),
  exitAppFullscreen: vi.fn(() => false),
}));
vi.mock('./notifications', () => ({
  handleNotificationSSE: vi.fn(),
  markReadOptimistic: vi.fn(),
  viewNotification: vi.fn(() => Promise.resolve()),
  loadUnreadNotifications: vi.fn(),
  loadNotifications: vi.fn(),
}));
vi.mock('./menu', () => ({ switchMenuItem: vi.fn() }));
vi.mock('../../api/client', () => ({ API_BASE: 'http://test', API: 'http://test/api/v1' }));
vi.mock('./devices', () => ({ getDeviceId: () => 'dev-test' }));

import { dismissToast, focusedThreadId, showToast, toasts, unreadNotifications } from '../store';
import {
  _resetToastLifetimeForTesting,
  dropAllNotificationToasts,
  dropNotificationToast,
  handleNotificationToastRequested,
  installNotificationToastLifetime,
  notificationToastKey,
  toastsToDrop,
} from './in-app-notification-toast';

const OVERFLOW_KEY = 'notifications-overflow';

function unreadSet(...ids: string[]): void {
  unreadNotifications.value = {
    status: 'loaded',
    data: ids.map((id) => ({
      id,
      title: 't',
      message: 'm',
      created_at: '2026-09-20T00:00:00Z',
      read: false,
    }) as Notification),
  };
}

/** Land a toast the way the engine's push-suppressed branch does. */
function arrive(id: string): void {
  handleNotificationToastRequested({
    notification_id: id,
    title: 'Claude is asking',
    body: 'Pick one',
    thread_id: 't-other',
    event_id: null,
    app_id: null,
    tap: null,
    sent_at_ms: Date.now(),
  });
}

function toastFor(id: string) {
  return toasts.value.find((t) => t.key === notificationToastKey(id));
}

function overflow() {
  return toasts.value.find((t) => t.key === OVERFLOW_KEY);
}

beforeEach(() => {
  toasts.value = [];
  focusedThreadId.value = null;
  unreadNotifications.value = { status: 'not-loaded' };
  _resetToastLifetimeForTesting();
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
  Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
  vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(null, { status: 200 }))));
  installNotificationToastLifetime();
});

describe('toastsToDrop', () => {
  it('names the ids the unread set just lost', () => {
    expect(toastsToDrop(['a', 'b', 'c'], ['b'])).toEqual(['a', 'c']);
  });

  it('names nothing when the set is unchanged', () => {
    expect(toastsToDrop(['a', 'b'], ['a', 'b'])).toEqual([]);
  });

  it('names nothing for an id the set only just gained', () => {
    expect(toastsToDrop(['a'], ['a', 'b'])).toEqual([]);
  });
});

describe('a read drops the toast', () => {
  it('drops it when the reader reaches the tap target', () => {
    unreadSet('n-1');
    arrive('n-1');
    expect(toastFor('n-1')).toBeTruthy();

    // What the seen target rule does: `markReadOptimistic` drops the row from
    // the unread set, which is what the badge falls on.
    unreadSet();

    expect(toastFor('n-1')).toBeUndefined();
  });

  it('leaves the toasts of rows that are still unread', () => {
    unreadSet('n-1', 'n-2');
    arrive('n-1');
    arrive('n-2');

    unreadSet('n-2');

    expect(toastFor('n-1')).toBeUndefined();
    expect(toastFor('n-2')).toBeTruthy();
  });

  it('clears every notification toast on Mark all read', () => {
    unreadSet('n-1', 'n-2', 'n-3');
    for (const id of ['n-1', 'n-2', 'n-3']) arrive(id);

    unreadSet();

    expect(toasts.value).toHaveLength(0);
  });
});

describe('a read the unread set cannot report', () => {
  // The `NotificationRead` arm of `handleGlobalEvent` is what calls these. It
  // is slower than the set by a round trip, and it answers where the set
  // cannot: the row was never in it to leave it.
  it('drops a toast whose row the set never carried', () => {
    arrive('n-1');
    unreadSet('n-other');
    expect(toastFor('n-1')).toBeTruthy();

    dropNotificationToast('n-1');

    expect(toastFor('n-1')).toBeUndefined();
  });

  it('unfolds a read notification the set never carried', () => {
    const six = ['n-1', 'n-2', 'n-3', 'n-4', 'n-5', 'n-6'];
    for (const id of six) arrive(id);
    expect(overflow()!.message).toBe('+2 more notifications');

    dropNotificationToast('n-5');

    expect(overflow()!.message).toBe('+1 more notification');
  });

  it('clears the lot on an all-read, individual toasts and the pile', () => {
    for (const id of ['n-1', 'n-2', 'n-3', 'n-4', 'n-5']) arrive(id);
    expect(overflow()).toBeTruthy();

    dropAllNotificationToasts();

    expect(toasts.value).toHaveLength(0);
  });

  it('leaves a toast raised by something other than a notification', () => {
    arrive('n-1');
    showToast('Applied 1 change', 'info', { key: 'apply-change' });

    dropAllNotificationToasts();

    expect(toasts.value.map((t) => t.key)).toEqual(['apply-change']);
  });
});

describe('a toast the page never held as unread', () => {
  it('survives a set that lands without it', () => {
    // The toast rides `NotificationToastRequested` while the unread reload
    // `NotificationCreated` kicked off is still in flight. Bare absence is
    // therefore not a read, and dropping on it would hide a fresh notification.
    arrive('n-1');
    unreadSet('n-other');

    expect(toastFor('n-1')).toBeTruthy();
  });

  it('survives a baseline that never named it', () => {
    unreadSet('n-other');
    arrive('n-1');
    unreadSet('n-other');

    expect(toastFor('n-1')).toBeTruthy();
  });
});

describe('a set that is not loaded', () => {
  it('drops nothing and forgets nothing', () => {
    unreadSet('n-1');
    arrive('n-1');

    unreadNotifications.value = { status: 'not-loaded' };
    expect(toastFor('n-1')).toBeTruthy();

    unreadNotifications.value = { status: 'loading' };
    expect(toastFor('n-1')).toBeTruthy();

    // The baseline survived, so the read that follows is still a transition.
    unreadSet();
    expect(toastFor('n-1')).toBeUndefined();
  });
});

describe('the overflow toast counts what is still unread', () => {
  const six = ['n-1', 'n-2', 'n-3', 'n-4', 'n-5', 'n-6'];

  function fold(): void {
    unreadSet(...six);
    for (const id of six) arrive(id);
  }

  it('folds the fifth and sixth', () => {
    fold();
    expect(overflow()!.message).toBe('+2 more notifications');
  });

  it('counts down as a folded notification is read', () => {
    fold();
    unreadSet('n-1', 'n-2', 'n-3', 'n-4', 'n-6');
    expect(overflow()!.message).toBe('+1 more notification');
  });

  it('goes away once every folded notification is read', () => {
    fold();
    unreadSet('n-1', 'n-2', 'n-3', 'n-4');
    expect(overflow()).toBeUndefined();
  });

  it('ignores a read of a notification it never folded', () => {
    fold();
    unreadSet('n-2', 'n-3', 'n-4', 'n-5', 'n-6');
    expect(overflow()!.message).toBe('+2 more notifications');
    expect(toastFor('n-1')).toBeUndefined();
  });

  it('stays gone once the reader has cleared it', () => {
    // The pile outlives the toast, so a read that decremented it blindly would
    // raise a fresh one. A read must never put a toast back on screen: that is
    // the inverse of the whole rule. The X and the toast's own tap both land
    // here, and the tap opens the panel the new toast would cover.
    fold();
    dismissToast(OVERFLOW_KEY);

    unreadSet('n-1', 'n-2', 'n-3', 'n-4', 'n-6');

    expect(overflow()).toBeUndefined();
  });

  it('starts the next pile from one', () => {
    fold();
    unreadSet('n-1', 'n-2', 'n-3', 'n-4');
    expect(overflow()).toBeUndefined();

    unreadSet('n-1', 'n-2', 'n-3', 'n-4', 'n-7');
    arrive('n-7');

    expect(overflow()!.message).toBe('+1 more notification');
  });
});
