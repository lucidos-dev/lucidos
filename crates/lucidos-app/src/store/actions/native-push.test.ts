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
let listenHandler: ((e: { payload: Record<string, unknown> }) => void) | null = null;
const listenUnlisten = vi.fn();
const listen = vi.fn((_event: string, handler: (e: { payload: Record<string, unknown> }) => void) => {
  listenHandler = handler;
  return Promise.resolve(listenUnlisten);
});
vi.mock('../../utils/tauri', () => ({
  showNativeNotification: (opts: { title: string; body: string; deepLink: Record<string, unknown> }) =>
    showNativeNotification(opts),
  listen: (event: string, handler: (e: { payload: Record<string, unknown> }) => void) =>
    listen(event, handler),
}));

const dispatchDeepLink = vi.fn();
vi.mock('./in-app-notification-toast', () => ({
  dispatchDeepLink: (...args: unknown[]) => dispatchDeepLink(...args),
}));

import {
  handleNativePushRequested,
  setupNativePushTapRouting,
  NATIVE_PUSH_STALE_AFTER_MS,
  type NativePushRequestedPayload,
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

describe('setupNativePushTapRouting → dispatchDeepLink', () => {
  beforeEach(() => {
    isTauri.mockReturnValue(true);
    listenHandler = null;
    listen.mockClear();
    dispatchDeepLink.mockClear();
  });

  it('registers a Tauri listener and routes a tap through dispatchDeepLink', async () => {
    await setupNativePushTapRouting();
    expect(listen).toHaveBeenCalledWith('native-notification-tapped', expect.any(Function));
    expect(listenHandler).toBeTruthy();

    // Simulate the Rust command emitting a tap with the SW-message shape.
    listenHandler!({
      payload: { notification_id: 'n-9', thread_id: 't-2', event_id: 'e-2', tap: { kind: 'modal' } },
    });
    expect(dispatchDeepLink).toHaveBeenCalledWith(
      expect.objectContaining({ notification: 'n-9', thread: 't-2', event: 'e-2' }),
    );
  });

  it('does not register a listener outside Tauri', async () => {
    isTauri.mockReturnValue(false);
    const un = await setupNativePushTapRouting();
    expect(listen).not.toHaveBeenCalled();
    expect(typeof un).toBe('function');
  });
});
