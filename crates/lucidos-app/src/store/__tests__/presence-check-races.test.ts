/**
 * The two interleavings that used to leave the reader with no surface at all.
 *
 * Each test drives the SSE dispatch in the order the frames arrive, with the
 * page's state changing between them in an explicit step. No timer decides the
 * order. See system-knowhow/notifications.md §3 and §4.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { EventStream } from '@lucidos/event-stream';
import { toasts, focusedThreadId } from '../store';

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
  exitAppFullscreen: vi.fn(() => false),
}));
vi.mock('../actions/notifications', () => ({
  handleNotificationSSE: vi.fn(),
  markReadOptimistic: vi.fn(),
  viewNotification: vi.fn(() => Promise.resolve()),
  loadUnreadNotifications: vi.fn(),
  loadNotifications: vi.fn(),
}));
vi.mock('../actions/menu', () => ({ switchMenuItem: vi.fn() }));
vi.mock('../../components/chat/scrollState', () => ({ isEventInViewport: vi.fn(() => false) }));
vi.mock('../../api/client', () => ({ API_BASE: 'http://test', API: 'http://test/api/v1' }));
vi.mock('../actions/devices', () => ({ getDeviceId: () => 'dev-test' }));
const showNativeNotification = vi.fn((_opts: unknown) => Promise.resolve());
vi.mock('../../utils/tauri', () => ({
  showNativeNotification: (opts: unknown) => showNativeNotification(opts),
  dismissNativeNotification: vi.fn(() => Promise.resolve()),
  takePendingNativeTaps: vi.fn(() => Promise.resolve([])),
  getNativeWindowActive: vi.fn(() => Promise.resolve(true)),
  listen: vi.fn(() => Promise.resolve(() => {})),
}));
vi.mock('../../utils/liveness', () => ({ postClientLog: vi.fn() }));

import * as scrollState from '../../components/chat/scrollState';
import { provideScrollState } from '../../components/chat/scrollStateHandle';
import { handleGlobalEvent } from '../actions/thread-sync';
import { setEventStream } from '../actions/event-stream';
import { setNativeWindowActive } from '../../utils/nativeWindow';

provideScrollState(scrollState);

const content = (notificationId: string) => ({
  notification_id: notificationId,
  title: 'Claude is asking',
  body: 'Pick one',
  thread_id: 't-elsewhere',
  event_id: null,
  app_id: null,
  tap: null,
  sent_at_ms: Date.now(),
});

const presenceCheck = (notificationId: string) => ({
  ...content(notificationId),
  deadline_ms: 2000,
});

function setVisibility(state: 'visible' | 'hidden'): void {
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: state });
}

const toastKeys = () => toasts.value.map((t) => t.key);

/** A transport that records which toasts were on screen as each pong left. */
function recordingTransport() {
  const onScreenAtPong: Array<Array<string | undefined>> = [];
  const submitPong = vi.fn(() => {
    onScreenAtPong.push(toastKeys());
  });
  const stream: EventStream = { submitPong, close: () => {}, ownsReconnect: false, revalidate: () => 'live', restart: () => {} };
  setEventStream(stream);
  return { submitPong, onScreenAtPong };
}

describe('the page acts on a PresenceCheck before it answers', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    setVisibility('visible');
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => true });
    setNativeWindowActive(true);
    showNativeNotification.mockClear();
  });

  afterEach(() => {
    setEventStream(null);
    setNativeWindowActive(true);
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it('a page that hides right after answering keeps the toast it answered for', () => {
    const { submitPong, onScreenAtPong } = recordingTransport();

    handleGlobalEvent('PresenceCheck', presenceCheck('n-race'));

    expect(submitPong).toHaveBeenCalledOnce();
    expect(submitPong.mock.calls[0]).toEqual(['n-race', { device_id: 'dev-test', outcome: 'shown' }]);
    expect(onScreenAtPong[0]).toContain('notification-n-race');

    // The window the engine's decision used to leave open.
    setVisibility('hidden');

    expect(toastKeys()).toContain('notification-n-race');
  });

  it('a desktop window that turns active after answering gets a toast instead of a lost banner', async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    setNativeWindowActive(false);
    const { submitPong } = recordingTransport();

    handleGlobalEvent('PresenceCheck', presenceCheck('n-desk'));
    expect(submitPong.mock.calls[0]).toEqual(['n-desk', { device_id: 'dev-test', outcome: 'not_shown' }]);
    expect(toastKeys()).toEqual([]);

    // The reader brings the window forward before the push-allowed frame lands.
    setNativeWindowActive(true);
    handleGlobalEvent('NativePushRequested', content('n-desk'));
    await new Promise((r) => setTimeout(r, 0));

    expect(showNativeNotification).not.toHaveBeenCalled();
    expect(toastKeys()).toContain('notification-n-desk');
  });
});
