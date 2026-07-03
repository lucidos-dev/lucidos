import { describe, it, expect, beforeEach, vi } from 'vitest';

const isTauri = vi.fn(() => true);
vi.mock('../../utils/platform', () => ({
  isTauri: () => isTauri(),
}));

const isPageActive = vi.fn(() => false);
vi.mock('../../utils/pageActive', () => ({
  isPageActive: () => isPageActive(),
}));

const showNativeNotification = vi.fn(
  (_opts: { title: string; body: string; deepLink: Record<string, unknown> }) => Promise.resolve(),
);
const dismissNativeNotification = vi.fn((_opts: { notificationId: string | null }) => Promise.resolve());
// Drain stub: defaults to empty; per-test override with mockResolvedValueOnce.
const takePendingNativeTaps = vi.fn((): Promise<Record<string, unknown>[]> => Promise.resolve([]));
let listenHandler: ((e: { payload: Record<string, unknown> }) => void) | null = null;
const listenUnlisten = vi.fn();
const listen = vi.fn((_event: string, handler: (e: { payload: Record<string, unknown> }) => void) => {
  listenHandler = handler;
  return Promise.resolve(listenUnlisten);
});
vi.mock('../../utils/tauri', () => ({
  showNativeNotification: (opts: { title: string; body: string; deepLink: Record<string, unknown> }) =>
    showNativeNotification(opts),
  dismissNativeNotification: (opts: { notificationId: string | null }) =>
    dismissNativeNotification(opts),
  takePendingNativeTaps: () => takePendingNativeTaps(),
  listen: (event: string, handler: (e: { payload: Record<string, unknown> }) => void) =>
    listen(event, handler),
}));

const dispatchDeepLink = vi.fn();
vi.mock('./in-app-notification-toast', () => ({
  dispatchDeepLink: (...args: unknown[]) => dispatchDeepLink(...args),
}));

// Breadcrumb telemetry — assert nothing, just keep it from hitting the network.
vi.mock('../../utils/liveness', () => ({
  postClientLog: vi.fn(),
}));

import {
  handleNativePushRequested,
  handleNativePushDismiss,
  setupNativePushTapRouting,
  NATIVE_PUSH_STALE_AFTER_MS,
  type NativePushRequestedPayload,
  type NativePushDismissRequestedPayload,
} from './native-push';

/** Fire a NativePushRequested the way the SSE channel would. The engine emits
 *  this only on the push-allowed branch, so it's the desktop OS surface. */
function emit(overrides: Partial<NativePushRequestedPayload> = {}): void {
  handleNativePushRequested({
    notification_id: overrides.notification_id ?? 'notif-1',
    title: overrides.title ?? 'Claude is asking',
    body: overrides.body ?? 'Pick one',
    thread_id: overrides.thread_id ?? null,
    event_id: overrides.event_id ?? null,
    app_id: overrides.app_id ?? null,
    tap: overrides.tap ?? null,
    sent_at_ms: overrides.sent_at_ms ?? Date.now(),
  });
}

/** showNativeBanner runs async (showNativeNotification await); let the
 *  microtask queue drain before asserting. */
const flush = () => new Promise((r) => setTimeout(r, 0));

describe('NativePushRequested → native desktop banner', () => {
  beforeEach(() => {
    isTauri.mockReturnValue(true);
    isPageActive.mockReturnValue(false);
    showNativeNotification.mockClear();
  });

  it('shows a banner and forwards the deep link in SW-message shape', async () => {
    emit({
      notification_id: 'n-7',
      title: 'Claude is asking',
      body: 'Pick one',
      thread_id: 't-1',
      event_id: 'e-1',
      tap: { kind: 'modal' },
    });
    await flush();
    expect(showNativeNotification).toHaveBeenCalledWith({
      title: 'Claude is asking',
      body: 'Pick one',
      deepLink: { notification_id: 'n-7', thread_id: 't-1', event_id: 'e-1', tap: { kind: 'modal' } },
    });
  });

  it('no-ops when not running in Tauri (browser / PWA get the real web push)', async () => {
    isTauri.mockReturnValue(false);
    emit();
    await flush();
    expect(showNativeNotification).not.toHaveBeenCalled();
  });

  it('drops a stale frame (late SSE-queue flush) past the freshness budget', async () => {
    emit({ sent_at_ms: Date.now() - NATIVE_PUSH_STALE_AFTER_MS - 1 });
    await flush();
    expect(showNativeNotification).not.toHaveBeenCalled();
  });

  it('no-ops when the page is active (OS surface is for non-active devices)', async () => {
    isPageActive.mockReturnValue(true);
    emit();
    await flush();
    expect(showNativeNotification).not.toHaveBeenCalled();
  });

  it('falls back to "Lucidos" when the title is empty', async () => {
    emit({ title: '', body: 'body only' });
    await flush();
    expect(showNativeNotification).toHaveBeenCalledWith(
      expect.objectContaining({ title: 'Lucidos', body: 'body only' }),
    );
  });
});

describe('setupNativePushTapRouting → drain → dispatchDeepLink', () => {
  beforeEach(() => {
    isTauri.mockReturnValue(true);
    listenHandler = null;
    listen.mockClear();
    dispatchDeepLink.mockClear();
    takePendingNativeTaps.mockClear();
    takePendingNativeTaps.mockResolvedValue([]); // default: nothing stashed
  });

  it('registers a Tauri listener and drains the stash (not the event payload)', async () => {
    await setupNativePushTapRouting();
    expect(listen).toHaveBeenCalledWith('native-notification-tapped', expect.any(Function));
    expect(listenHandler).toBeTruthy();
  });

  it('routes a tap stashed BEFORE the listener existed (cold / startup drain)', async () => {
    // The durable-delivery fix: a tap that fired while the page wasn't listening
    // is drained on startup, not lost. Default Tap::Modal shape.
    takePendingNativeTaps.mockResolvedValueOnce([
      { notification_id: 'n-9', thread_id: 't-2', event_id: 'e-2', tap: { kind: 'modal' } },
    ]);
    await setupNativePushTapRouting();
    await flush();
    expect(dispatchDeepLink).toHaveBeenCalledWith(
      expect.objectContaining({ notification: 'n-9', thread: 't-2', event: 'e-2' }),
    );
  });

  it('drains and routes on the live signal (warm path)', async () => {
    await setupNativePushTapRouting(); // startup drain empty
    await flush();
    expect(dispatchDeepLink).not.toHaveBeenCalled();

    // A subsequent tap stashes + wakes the signal; the listener drains it.
    takePendingNativeTaps.mockResolvedValueOnce([
      { notification_id: 'n-1', tap: { kind: 'modal' } },
    ]);
    listenHandler!({ payload: {} });
    await flush();
    expect(dispatchDeepLink).toHaveBeenCalledWith(
      expect.objectContaining({ notification: 'n-1' }),
    );
  });

  it('dispatches every tap when several were stashed', async () => {
    takePendingNativeTaps.mockResolvedValueOnce([
      { notification_id: 'a', tap: { kind: 'modal' } },
      { notification_id: 'b', tap: { kind: 'modal' } },
    ]);
    await setupNativePushTapRouting();
    await flush();
    expect(dispatchDeepLink).toHaveBeenCalledTimes(2);
  });

  // A native banner only ever shows while the page is INACTIVE
  // (handleNativePushRequested gates on isPageActive), so a banner tap ALWAYS
  // lands on a backgrounded/hidden — often JS-throttled — WKWebView.
  // show_main_window then just SHOWS the existing window (no reload), so the
  // startup cold drain never re-runs and the warm app.emit can be dropped on the
  // just-resumed webview. The window 'focus' / document 'visibilitychange' events
  // WebKit fires on show are the reliable, eval-independent drain triggers — this
  // is the "tap focuses the window but never deep-links" fix.
  it('drains and routes when the window regains focus (no-reload window-show path)', async () => {
    await setupNativePushTapRouting(); // startup drain empty
    await flush();
    dispatchDeepLink.mockClear();

    takePendingNativeTaps.mockResolvedValueOnce([
      {
        notification_id: 'n-focus',
        thread_id: 't-7',
        tap: { kind: 'navigate', to: { target: 'thread', id: 't-7' } },
      },
    ]);
    window.dispatchEvent(new Event('focus'));
    await flush();
    expect(dispatchDeepLink).toHaveBeenCalledWith(
      expect.objectContaining({ notification: 'n-focus', thread: 't-7' }),
    );
  });

  it('drains and routes when the page becomes visible again', async () => {
    await setupNativePushTapRouting(); // startup drain empty
    await flush();
    dispatchDeepLink.mockClear();

    takePendingNativeTaps.mockResolvedValueOnce([
      { notification_id: 'n-vis', tap: { kind: 'modal' } },
    ]);
    document.dispatchEvent(new Event('visibilitychange'));
    await flush();
    expect(dispatchDeepLink).toHaveBeenCalledWith(
      expect.objectContaining({ notification: 'n-vis' }),
    );
  });

  it('does NOT drain on a visibilitychange into the hidden state', async () => {
    await setupNativePushTapRouting();
    await flush();
    dispatchDeepLink.mockClear();
    takePendingNativeTaps.mockClear();

    const prev = document.visibilityState;
    try {
      (document as unknown as { visibilityState: string }).visibilityState = 'hidden';
      document.dispatchEvent(new Event('visibilitychange'));
      await flush();
      expect(takePendingNativeTaps).not.toHaveBeenCalled();
      expect(dispatchDeepLink).not.toHaveBeenCalled();
    } finally {
      (document as unknown as { visibilityState: string }).visibilityState = prev;
    }
  });

  it('an empty drain dispatches nothing', async () => {
    await setupNativePushTapRouting();
    await flush();
    expect(dispatchDeepLink).not.toHaveBeenCalled();
  });

  it('does not register a listener or drain outside Tauri', async () => {
    isTauri.mockReturnValue(false);
    const un = await setupNativePushTapRouting();
    expect(listen).not.toHaveBeenCalled();
    expect(takePendingNativeTaps).not.toHaveBeenCalled();
    expect(typeof un).toBe('function');
  });
});

/** Fire a NativePushDismissRequested the way the SSE channel would. Uses `in`
 *  (not `??`) so an explicit `notification_id: null` (dismiss-all) is preserved
 *  rather than coerced back to the default id. */
function emitDismiss(overrides: Partial<NativePushDismissRequestedPayload> = {}): void {
  handleNativePushDismiss({
    notification_id:
      'notification_id' in overrides ? (overrides.notification_id as string | null) : 'notif-1',
    sent_at_ms: overrides.sent_at_ms ?? Date.now(),
  });
}

describe('NativePushDismissRequested → remove native desktop banner', () => {
  beforeEach(() => {
    isTauri.mockReturnValue(true);
    isPageActive.mockReturnValue(false);
    dismissNativeNotification.mockClear();
  });

  it('removes one banner by id', async () => {
    emitDismiss({ notification_id: 'n-7' });
    await flush();
    expect(dismissNativeNotification).toHaveBeenCalledWith({ notificationId: 'n-7' });
  });

  it('removes all banners when notification_id is null (mark-all-read)', async () => {
    emitDismiss({ notification_id: null });
    await flush();
    expect(dismissNativeNotification).toHaveBeenCalledWith({ notificationId: null });
  });

  it('no-ops when not running in Tauri (web can\'t silently remove a push banner)', async () => {
    isTauri.mockReturnValue(false);
    emitDismiss();
    await flush();
    expect(dismissNativeNotification).not.toHaveBeenCalled();
  });

  it('drops a stale frame past the freshness budget (late dismiss-all guard)', async () => {
    emitDismiss({ notification_id: null, sent_at_ms: Date.now() - NATIVE_PUSH_STALE_AFTER_MS - 1 });
    await flush();
    expect(dismissNativeNotification).not.toHaveBeenCalled();
  });

  it('dismisses regardless of page-active state (no isPageActive gate)', async () => {
    isPageActive.mockReturnValue(true);
    emitDismiss({ notification_id: 'n-9' });
    await flush();
    expect(dismissNativeNotification).toHaveBeenCalledWith({ notificationId: 'n-9' });
  });
});
