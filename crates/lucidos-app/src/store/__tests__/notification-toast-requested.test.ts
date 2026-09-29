import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { toasts, focusedThreadId, preferences, TOAST_AUTO_DISMISS_MS } from '../store';

vi.mock('../actions/threads', () => ({
  focusThread: vi.fn(),
  unfocusThread: vi.fn(),
  focusThreadOrBootstrap: vi.fn(),
}));
vi.mock('../actions/apps', () => ({
  loadApps: vi.fn(),
  openAppById: vi.fn(),
  refreshAppUI: vi.fn(),
  captureAppUI: vi.fn(),
  // A `thread` navigate leaves fullscreen before it focuses, so the app the
  // reader is inside cannot hide the conversation the tap lands in.
  exitAppFullscreen: vi.fn(() => false),
}));
const markReadOptimistic = vi.fn();
vi.mock('../actions/notifications', () => ({
  handleNotificationSSE: vi.fn(),
  markReadOptimistic: (...args: unknown[]) => markReadOptimistic(...args),
  viewNotification: vi.fn(() => Promise.resolve()),
  loadUnreadNotifications: vi.fn(),
  loadNotifications: vi.fn(),
}));
const switchMenuItem = vi.fn();
vi.mock('../actions/menu', () => ({
  switchMenuItem: (...args: unknown[]) => switchMenuItem(...args),
}));
const isEventInViewport = vi.fn().mockReturnValue(false);
vi.mock('../../components/chat/scrollState', () => ({
  isEventInViewport: (...args: unknown[]) => isEventInViewport(...args),
}));
vi.mock('../../api/client', () => ({ API_BASE: 'http://test', API: 'http://test/api/v1' }));
vi.mock('../actions/devices', () => ({ getDeviceId: () => 'dev-test' }));

import type { Tap } from '@lucidos/sdk';
import { handleGlobalEvent } from '../actions/thread-sync';
import { focusThreadOrBootstrap } from '../actions/threads';
import { viewNotification, handleNotificationSSE } from '../actions/notifications';
import {
  handleNotificationToastRequested,
  TOAST_REQUEST_STALE_AFTER_MS,
} from '../actions/in-app-notification-toast';

/** Fire a NotificationToastRequested the way the SSE channel would. This is
 *  the §4 in-app surface trigger — the engine emits it only after it decides
 *  to suppress the OS push, so the toast and the push are mutually exclusive
 *  (see notifications.md §4). */
function emitToast(overrides: Partial<{
  notification_id: string; title: string; body: string;
  thread_id: string; event_id: string; app_id: string; tap: Tap;
  sent_at_ms: number;
}> = {}): void {
  handleNotificationToastRequested({
    notification_id: overrides.notification_id ?? `notif-${Math.random().toString(36).slice(2, 10)}`,
    title: overrides.title ?? 'Claude is asking',
    body: overrides.body ?? 'Pick one',
    thread_id: overrides.thread_id ?? 't-default',
    event_id: overrides.event_id ?? null,
    app_id: overrides.app_id ?? null,
    tap: overrides.tap ?? null,
    sent_at_ms: overrides.sent_at_ms ?? Date.now(),
  });
}

describe('NotificationToastRequested (active page) → in-app toast', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    isEventInViewport.mockReset().mockReturnValue(false);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(null, { status: 200 }))));
    vi.clearAllMocks();
  });

  it('maps the title and body straight onto the toast title and message', () => {
    emitToast({ notification_id: 'notif-1', title: 'Claude is asking', body: 'Pick one' });

    const t = toasts.value[0];
    expect(t).toBeTruthy();
    expect(t.title).toBe('Claude is asking');
    expect(t.message).toBe('Pick one');
    expect(t.type).toBe('info');
    expect(t.key).toBe('notification-notif-1');
  });

  it('opens a modal (pure) notification on a tap anywhere on the card, with no button', () => {
    // Plain notification (no app/thread, no tap): resolveDeepLink returns
    // view-notification. The card tap opens the detail and marks read. There is
    // no button and no [OK]: deferring is the X (dismiss, keep unread), and
    // merely showing the toast never marks it read.
    emitToast({ notification_id: 'notif-plain' });

    const t = toasts.value[0];
    expect(t).toBeTruthy();
    expect(t.onClick).toBeTypeOf('function');
    expect(t.action).toBeUndefined();
    expect(t.secondaryAction).toBeUndefined();
    // The X stays available so the user can defer.
    expect(t.dismissable).not.toBe(false);
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('a tap on a modal notification opens the detail and marks it read', async () => {
    emitToast({ notification_id: 'notif-open' });
    const t = toasts.value[0];
    t.onClick!();
    expect(viewNotification).toHaveBeenCalledWith('notif-open');
    // The read is guaranteed by the handler (after viewNotification settles),
    // not left to viewNotification's fetch-contingent internal mark.
    await Promise.resolve();
    expect(markReadOptimistic).toHaveBeenCalledWith('notif-open');
  });

  it('a tap on a modal notification still marks it read when the detail fetch fails', async () => {
    // viewNotification swallows a failed fetch and does NOT mark read. The toast
    // is already dismissed, so the tap must mark read itself, or the row
    // silently stays unread.
    vi.mocked(viewNotification).mockReturnValueOnce(Promise.reject(new Error('fetch failed')));
    emitToast({ notification_id: 'notif-openfail' });
    const t = toasts.value[0];
    t.onClick!();
    await Promise.resolve();
    await Promise.resolve();
    expect(markReadOptimistic).toHaveBeenCalledWith('notif-openfail');
  });

  it('a double tap opens once', () => {
    emitToast({ notification_id: 'notif-double' });
    const t = toasts.value[0];
    t.onClick!();
    t.onClick!();
    expect(viewNotification).toHaveBeenCalledTimes(1);
  });

  it('uses notification-<id> as the toast key so retries share one slot', () => {
    emitToast({ notification_id: 'notif-42' });
    emitToast({ notification_id: 'notif-42' });

    const matching = toasts.value.filter(t => t.key === 'notification-notif-42');
    expect(matching).toHaveLength(1);
  });

  it('falls back to "Lucidos" when title is empty', () => {
    emitToast({ notification_id: 'notif-3', title: '', body: 'body only' });
    expect(toasts.value[0].title).toBe('Lucidos');
    expect(toasts.value[0].message).toBe('body only');
  });

  it('shows a body-less notification as its title alone, untitled', () => {
    emitToast({ notification_id: 'notif-4', title: 'Backup complete', body: '' });
    expect(toasts.value[0].title).toBeUndefined();
    expect(toasts.value[0].message).toBe('Backup complete');
  });

  it('keeps a multi-paragraph body whole, under the title', () => {
    const body = 'No release candidate tonight.\n\nWhy: 17 files are deleted on main.';
    emitToast({ notification_id: 'notif-5', title: 'Nightly release prep: skipped', body });
    expect(toasts.value[0].title).toBe('Nightly release prep: skipped');
    expect(toasts.value[0].message).toBe(body);
  });

  it('opens a navigate (app-CTA) notification on a card tap, with no button', () => {
    emitToast({
      notification_id: 'notif-4',
      title: 'Time to check in',
      body: 'Log today',
      app_id: 'habit-tracker',
      tap: { kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker' } },
    });

    const t = toasts.value[0];
    expect(t).toBeTruthy();
    expect(t.onClick).toBeTypeOf('function');
    expect(t.action).toBeUndefined();
    expect(t.secondaryAction).toBeUndefined();
  });

  it('a tap on a navigate (thread + event) notification deep-links to the source event', () => {
    // The tap must route through the SAME navigate dispatch the inbox detail
    // and push taps use: focusThreadOrBootstrap(threadId, { targetEventId }).
    // The scroll-and-pulse itself is covered by e2e/notifications.spec.ts.
    emitToast({
      notification_id: 'notif-q',
      title: 'Claude is asking',
      body: 'Ship it?',
      thread_id: 't-9',
      event_id: 'e-7',
      tap: { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } },
    });

    toasts.value[0].onClick!();

    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('t-9', { targetEventId: 'e-7' });
    expect(markReadOptimistic).toHaveBeenCalledWith('notif-q');
  });

  it('defers a navigate notification via the X (dismissable, keeps it unread), with no OK button', () => {
    // The only "clear without opening" path is the toast's built-in X, which
    // dismisses WITHOUT marking read. An [OK] that marks read without opening
    // would bury an unanswered question.
    emitToast({
      notification_id: 'notif-defer',
      thread_id: 't-9',
      event_id: 'e-7',
      tap: { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } },
    });

    const t = toasts.value[0];
    expect(t.onClick).toBeTypeOf('function');
    expect(t.action).toBeUndefined();
    expect(t.secondaryAction).toBeUndefined();
    expect(t.dismissable).not.toBe(false);
    expect(markReadOptimistic).not.toHaveBeenCalled();
    expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
  });

  it('coerces a historical tap=none into a modal toast the card tap opens (not passive/auto-read)', () => {
    // tap=none is retired (docs/plans/2026-07-02-remove-notification-tap-none.md).
    // A historical/coerced none behaves like modal: a toast that persists and
    // is NOT auto-marked-read on show. Every notification is openable.
    emitToast({ notification_id: 'notif-legacy-none', tap: { kind: 'none' } as unknown as Tap });

    const t = toasts.value[0];
    expect(t).toBeTruthy();
    expect(t.onClick).toBeTypeOf('function');
    expect(t.secondaryAction).toBeUndefined();
    expect(t.dismissable).not.toBe(false);
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });
});

describe('NotificationToastRequested → freshness gate', () => {
  // The engine emits this only on the push-suppressed branch, so there's no
  // OS push to collide with. But an iOS PWA buffers SSE while JS is suspended;
  // a queued toast that flushes long after the user resumed would pop on top
  // of whatever they're now doing. Drop the stale one — the bell badge
  // (NotificationCreated) already reflects it.
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    isEventInViewport.mockReset().mockReturnValue(false);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    vi.clearAllMocks();
  });

  it('drops a toast that arrives past the staleness budget', () => {
    emitToast({
      notification_id: 'notif-stale',
      sent_at_ms: Date.now() - TOAST_REQUEST_STALE_AFTER_MS - 100,
    });
    expect(toasts.value).toHaveLength(0);
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('renders a toast that arrives within the staleness budget', () => {
    emitToast({
      notification_id: 'notif-fresh',
      sent_at_ms: Date.now() - 100,
    });
    expect(toasts.value).toHaveLength(1);
  });
});

describe('NotificationToastRequested → §4 row classification (suppression when user has seen the event)', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    isEventInViewport.mockReset().mockReturnValue(false);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(null, { status: 200 }))));
    vi.clearAllMocks();
  });

  it('skips toast and auto-marks-read when focused thread + tab visible + event in viewport', () => {
    focusedThreadId.value = 'thread-A';
    isEventInViewport.mockReturnValue(true);

    emitToast({ notification_id: 'notif-vis', thread_id: 'thread-A', event_id: 'evt-1' });

    expect(toasts.value).toHaveLength(0);
    expect(markReadOptimistic).toHaveBeenCalledWith('notif-vis');
  });

  it('shows the toast when focused thread + event NOT in viewport (scrolled away)', () => {
    focusedThreadId.value = 'thread-A';
    isEventInViewport.mockReturnValue(false);

    emitToast({ notification_id: 'notif-scroll', thread_id: 'thread-A', event_id: 'evt-2' });

    expect(toasts.value).toHaveLength(1);
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('shows the toast when focused on a DIFFERENT thread', () => {
    focusedThreadId.value = 'thread-B';
    isEventInViewport.mockReturnValue(true);

    emitToast({ notification_id: 'notif-other', thread_id: 'thread-A', event_id: 'evt-3' });

    expect(toasts.value).toHaveLength(1);
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('shows the toast when the notification has no event_id (cannot prove user saw it)', () => {
    focusedThreadId.value = 'thread-A';
    isEventInViewport.mockReturnValue(true);

    emitToast({ notification_id: 'notif-no-evt', thread_id: 'thread-A' });

    expect(toasts.value).toHaveLength(1);
    expect(isEventInViewport).not.toHaveBeenCalled();
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('does NOT render a toast when the page is hidden (Row 4)', () => {
    // The engine broadcasts NotificationToastRequested; a hidden page receives
    // it but must stay silent — bell badge only (driven by NotificationCreated).
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });

    emitToast({ notification_id: 'notif-hidden', thread_id: 'thread-A', event_id: 'evt-9' });

    expect(toasts.value).toHaveLength(0);
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });
});

describe('SSE wiring: handleGlobalEvent routes NotificationToastRequested → toast', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    isEventInViewport.mockReset().mockReturnValue(false);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(null, { status: 200 }))));
    vi.clearAllMocks();
  });

  it('renders the toast from the SSE frame payload', () => {
    handleGlobalEvent('NotificationToastRequested', {
      notification_id: 'notif-sse',
      title: 'From SSE',
      body: 'rendered inline',
      thread_id: 't-source',
      event_id: null,
      app_id: null,
      tap: null,
      sent_at_ms: Date.now(),
    });

    const t = toasts.value.find(x => x.key === 'notification-notif-sse');
    expect(t).toBeTruthy();
    expect(t!.title).toBe('From SSE');
    expect(t!.message).toBe('rendered inline');
  });
});

describe('NotificationCreated SSE no longer fires toasts (architecture invariant)', () => {
  // The iOS PWA queueing fix: NotificationCreated SSE is bell-badge-only.
  // The toast is driven by NotificationToastRequested (which has a freshness
  // gate AND is only emitted on the push-suppressed branch). Without this
  // invariant, an iOS-queued NotificationCreated would flush after the user
  // taps the OS push and leak a duplicate in-app toast.
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    isEventInViewport.mockReset().mockReturnValue(false);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    vi.clearAllMocks();
  });

  it('does NOT render a toast even when the page would otherwise qualify', () => {
    focusedThreadId.value = 't-other';
    handleGlobalEvent('NotificationCreated', {
      id: 'notif-from-sse',
      title: 'Should not render',
      message: 'as a toast anymore',
      thread_id: 't-source',
      event_id: 'evt-1',
    });
    expect(toasts.value).toHaveLength(0);
  });
});

describe('NotificationToastRequested → overflow at 5+ individual toasts', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    isEventInViewport.mockReset().mockReturnValue(false);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(null, { status: 200 }))));
    vi.clearAllMocks();
  });

  it('keeps the first 4 individual toasts and rolls the 5th into "+1 more"', () => {
    for (let i = 1; i <= 5; i++) emitToast({ notification_id: `n-${i}` });

    const individuals = toasts.value.filter(t => t.key?.startsWith('notification-'));
    expect(individuals).toHaveLength(4);
    // New toasts prepend (newest at the top of the column-stacked container), so
    // the kept four are the first four created, listed newest-first.
    expect(individuals.map(t => t.key)).toEqual([
      'notification-n-4', 'notification-n-3', 'notification-n-2', 'notification-n-1',
    ]);

    const overflow = toasts.value.find(t => t.key === 'notifications-overflow');
    expect(overflow).toBeTruthy();
    expect(overflow!.message).toBe('+1 more notification');
    // Single-action toast: click the text to view, no separate button.
    expect(overflow!.action).toBeUndefined();
    expect(overflow!.onClick).toBeTypeOf('function');
  });

  it('increments the overflow count on further notifications', () => {
    for (let i = 1; i <= 7; i++) emitToast({ notification_id: `n-${i}` });

    const overflow = toasts.value.find(t => t.key === 'notifications-overflow');
    expect(overflow).toBeTruthy();
    expect(overflow!.message).toBe('+3 more notifications');

    // Individual slots stay pinned to the original 4
    expect(toasts.value.filter(t => t.key?.startsWith('notification-'))).toHaveLength(4);
  });

  it('opens the notifications panel and clears the overflow toast when the text is clicked', () => {
    for (let i = 1; i <= 6; i++) emitToast({ notification_id: `n-${i}` });
    const overflow = toasts.value.find(t => t.key === 'notifications-overflow')!;

    overflow.onClick!();

    expect(switchMenuItem).toHaveBeenCalledWith('notifications');
    expect(toasts.value.find(t => t.key === 'notifications-overflow')).toBeUndefined();
  });

  it('restarts counting from 1 after the overflow toast is dismissed', () => {
    for (let i = 1; i <= 6; i++) emitToast({ notification_id: `n-${i}` });
    const overflow = toasts.value.find(t => t.key === 'notifications-overflow')!;
    overflow.onClick!();

    emitToast({ notification_id: 'n-7' });

    const next = toasts.value.find(t => t.key === 'notifications-overflow');
    expect(next).toBeTruthy();
    expect(next!.message).toBe('+1 more notification');
  });

  it('does not route into overflow when the new notification is suppressed (user saw event)', () => {
    for (let i = 1; i <= 4; i++) emitToast({ notification_id: `n-${i}` });
    focusedThreadId.value = 'thread-A';
    isEventInViewport.mockReturnValue(true);

    emitToast({ notification_id: 'n-5', thread_id: 'thread-A', event_id: 'evt-X' });

    expect(toasts.value.find(t => t.key === 'notifications-overflow')).toBeUndefined();
    expect(toasts.value.filter(t => t.key?.startsWith('notification-'))).toHaveLength(4);
    expect(markReadOptimistic).toHaveBeenCalledWith('n-5');
  });
});

describe('notification toasts persist (no auto-dismiss)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    toasts.value = [];
    focusedThreadId.value = null;
    isEventInViewport.mockReset().mockReturnValue(false);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(null, { status: 200 }))));
    vi.clearAllMocks();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('leaves an actioned (tap.kind=navigate) toast sticky', () => {
    // Guards against a future "auto-dismiss every keyed toast" overcorrection
    // so the card tap (and the X) stay reachable for the user to act.
    emitToast({
      notification_id: 'cta-stick',
      app_id: 'habit-tracker',
      tap: { kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker' } },
    });

    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS * 2);
    expect(toasts.value.find(t => t.key === 'notification-cta-stick')).toBeTruthy();
  });

  it('leaves a modal (pure) notification toast sticky so its card tap stays usable', () => {
    emitToast({ notification_id: 'modal-stick' });

    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS * 2);
    expect(toasts.value.find(t => t.key === 'notification-modal-stick')).toBeTruthy();
  });
});

describe('notification_toasts = false silences the pop-up', () => {
  // The workspace-wide switch behind Settings → Notifications → In-app toasts.
  // Off, the notification waits on the bell badge and in the panel instead of
  // interrupting. Every other block in this file leaves the preference unloaded,
  // which is the default-on path, so those cover the unchanged half.
  function setToastsPreference(value: string): void {
    preferences.value = { status: 'loaded', data: { notification_toasts: value } };
  }

  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    localStorage.removeItem('lucidos-notification-toasts');
    isEventInViewport.mockReset().mockReturnValue(false);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(null, { status: 200 }))));
    vi.clearAllMocks();
  });
  afterEach(() => {
    preferences.value = { status: 'not-loaded' };
    localStorage.removeItem('lucidos-notification-toasts');
  });

  it('renders no toast for a Row 2 / 3 notification', () => {
    setToastsPreference('false');
    focusedThreadId.value = 'thread-B';

    emitToast({ notification_id: 'notif-quiet', thread_id: 'thread-A', event_id: 'evt-1' });

    expect(toasts.value).toHaveLength(0);
    // Deferred, not dismissed: the row has to stay unread for the panel.
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('renders no overflow toast either', () => {
    setToastsPreference('false');
    for (let i = 1; i <= 7; i++) emitToast({ notification_id: `quiet-${i}` });

    expect(toasts.value).toHaveLength(0);
  });

  it('still bumps the bell badge, which is a different SSE handler', () => {
    // The gate lives in showInAppNotificationToast, below the badge's own path.
    // Silencing the toast must not silence the count the panel is found by.
    setToastsPreference('false');
    handleGlobalEvent('NotificationCreated', {
      id: 'notif-badge',
      title: 'Still counted',
      message: 'on the bell',
      thread_id: 't-source',
      event_id: 'evt-1',
    });

    expect(handleNotificationSSE).toHaveBeenCalled();
    expect(toasts.value).toHaveLength(0);
  });

  it('still auto-marks read when the user is looking at the source event (Row 1)', () => {
    // Row 1 is about attention, not about the toast, so the switch leaves it be.
    setToastsPreference('false');
    focusedThreadId.value = 'thread-A';
    isEventInViewport.mockReturnValue(true);

    emitToast({ notification_id: 'notif-seen', thread_id: 'thread-A', event_id: 'evt-2' });

    expect(toasts.value).toHaveLength(0);
    expect(markReadOptimistic).toHaveBeenCalledWith('notif-seen');
  });

  it('renders the toast again when the preference is explicitly true', () => {
    setToastsPreference('true');
    focusedThreadId.value = 'thread-B';

    emitToast({ notification_id: 'notif-loud', thread_id: 'thread-A', event_id: 'evt-3' });

    expect(toasts.value).toHaveLength(1);
  });

  it('holds while preferences are still loading, from the device-local mirror', () => {
    // The SSE stream connects before the preferences GET returns, and on an
    // iOS PWA that gap is a whole round trip. Answering "on" there pops the
    // exact toast the user turned off, so the last known value answers instead.
    localStorage.setItem('lucidos-notification-toasts', 'false');
    preferences.value = { status: 'loading' };
    focusedThreadId.value = 'thread-B';

    emitToast({ notification_id: 'notif-cold', thread_id: 'thread-A', event_id: 'evt-4' });

    expect(toasts.value).toHaveLength(0);
  });

  it('holds when the preferences load failed, which never resolves on its own', () => {
    localStorage.setItem('lucidos-notification-toasts', 'false');
    preferences.value = { status: 'failed', error: 'unreachable' };
    focusedThreadId.value = 'thread-B';

    emitToast({ notification_id: 'notif-failed', thread_id: 'thread-A', event_id: 'evt-5' });

    expect(toasts.value).toHaveLength(0);
  });

  it('lets the served value beat a stale mirror once preferences load', () => {
    localStorage.setItem('lucidos-notification-toasts', 'false');
    setToastsPreference('true');
    focusedThreadId.value = 'thread-B';

    emitToast({ notification_id: 'notif-fresh-on', thread_id: 'thread-A', event_id: 'evt-6' });

    expect(toasts.value).toHaveLength(1);
  });

  it('pops as before with no preference and no mirror, the untouched default', () => {
    preferences.value = { status: 'loading' };
    focusedThreadId.value = 'thread-B';

    emitToast({ notification_id: 'notif-default', thread_id: 'thread-A', event_id: 'evt-7' });

    expect(toasts.value).toHaveLength(1);
  });
});

// A read is the other end of the toast's lifetime. The unread-set watch owns
// the reads this page can see, in `actions/notification-toast-lifetime.test.ts`.
// These two SSE arms own the reads it cannot, and are a round trip slower.
describe('NotificationRead / NotificationsAllRead → the toast goes', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    isEventInViewport.mockReset().mockReturnValue(false);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(null, { status: 200 }))));
    vi.clearAllMocks();
  });

  it('drops the read row\'s toast and leaves the others', () => {
    emitToast({ notification_id: 'n-read' });
    emitToast({ notification_id: 'n-keep' });

    handleGlobalEvent('NotificationRead', { id: 'n-read', actor: null });

    expect(toasts.value.map(t => t.key)).toEqual(['notification-n-keep']);
    // The badge still refreshes: the drop is additional, not a replacement.
    expect(handleNotificationSSE).toHaveBeenCalled();
  });

  it('counts the overflow down when the read row was folded into it', () => {
    for (let i = 1; i <= 6; i++) emitToast({ notification_id: `n-${i}` });
    expect(toasts.value.find(t => t.key === 'notifications-overflow')!.message)
      .toBe('+2 more notifications');

    handleGlobalEvent('NotificationRead', { id: 'n-6', actor: null });

    expect(toasts.value.find(t => t.key === 'notifications-overflow')!.message)
      .toBe('+1 more notification');
  });

  it('clears every notification toast on an all-read', () => {
    for (let i = 1; i <= 6; i++) emitToast({ notification_id: `n-${i}` });

    handleGlobalEvent('NotificationsAllRead', { actor: null });

    expect(toasts.value).toHaveLength(0);
    expect(handleNotificationSSE).toHaveBeenCalled();
  });

  it('ignores a read frame carrying no id', () => {
    emitToast({ notification_id: 'n-1' });

    handleGlobalEvent('NotificationRead', { actor: null });

    expect(toasts.value).toHaveLength(1);
  });
});
