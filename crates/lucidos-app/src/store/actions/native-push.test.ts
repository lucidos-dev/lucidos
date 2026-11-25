import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const acknowledge = vi.fn((_url: string, _init?: RequestInit) => Promise.resolve({ ok: true }));
vi.mock('../../api/client/_core', () => ({
  API: '/api/v1',
  json: (url: string, init?: RequestInit) => acknowledge(url, init),
}));

const isTauri = vi.fn(() => true);
vi.mock('../../utils/platform', () => ({
  isTauri: () => isTauri(),
}));


let workspaceId: string | null = 'myws';
vi.mock('../../utils/basePath', () => ({
  get WORKSPACE_ID() {
    return workspaceId;
  },
}));

const showNativeNotification = vi.fn(
  (_opts: { title: string; body: string; deepLink: Record<string, unknown> }): Promise<boolean | null> => Promise.resolve(true),
);
const dismissNativeNotification = vi.fn(
  (_opts: { workspace: string | null; notificationId: string | null }) => Promise.resolve(),
);
const takePendingNativeTaps = vi.fn(
  (_workspace: string | null): Promise<Record<string, unknown>[]> => Promise.resolve([]),
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
  dismissNativeNotification: (opts: { workspace: string | null; notificationId: string | null }) =>
    dismissNativeNotification(opts),
  takePendingNativeTaps: (workspace: string | null) => takePendingNativeTaps(workspace),
  listen: (event: string, handler: (e: { payload: Record<string, unknown> }) => void) =>
    listen(event, handler),
}));

const dispatchDeepLink = vi.fn();
const presentInAppNotification = vi.fn((_input: unknown) => 'not_shown');
vi.mock('./in-app-notification-toast', () => ({
  dispatchDeepLink: (...args: unknown[]) => dispatchDeepLink(...args),
  presentInAppNotification: (input: unknown) => presentInAppNotification(input),
}));

vi.mock('../../utils/liveness', () => ({
  postClientLog: vi.fn(),
}));

import { DEVICE_ID_KEY } from '../../utils/deviceIdKey';
import {
  handleNativePushRequested,
  handleNativePushDismiss,
  setupNativePushTapRouting,
  NATIVE_PUSH_STALE_AFTER_MS,
  type NativePushRequestedPayload,
  type NativePushDismissRequestedPayload,
} from './native-push';

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
    audience: overrides.audience ?? null,
  });
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe('NativePushRequested → native desktop banner', () => {
  beforeEach(() => {
    isTauri.mockReturnValue(true);
    presentInAppNotification.mockReset().mockReturnValue('not_shown');
    showNativeNotification.mockClear();
    acknowledge.mockClear();
  });

  it('acknowledges only after the OS accepts the banner', async () => {
    let accept!: () => void;
    showNativeNotification.mockImplementationOnce(() => new Promise<boolean | null>((resolve) => { accept = () => resolve(true); }));
    emit();
    await flush();
    expect(acknowledge).not.toHaveBeenCalled();
    accept();
    await flush();
    expect(acknowledge).toHaveBeenCalledTimes(1);
  });

  it('preserves successful legacy native completion', async () => {
    showNativeNotification.mockResolvedValueOnce(null);
    emit();
    await flush();
    expect(acknowledge).toHaveBeenCalledTimes(1);
  });

  it('does not acknowledge native delivery rejected by an updated client', async () => {
    showNativeNotification.mockResolvedValueOnce(false);
    emit();
    await flush();
    expect(acknowledge).not.toHaveBeenCalled();
  });

  it('leaves rejected banners pending for recovery', async () => {
    showNativeNotification.mockRejectedValueOnce(new Error('OS unavailable'));
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    emit();
    await flush();
    expect(acknowledge).not.toHaveBeenCalled();
    warning.mockRestore();
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
      deepLink: {
        notification_id: 'n-7',
        thread_id: 't-1',
        event_id: 'e-1',
        tap: { kind: 'modal' },
        workspace: 'myws',
      },
    });
  });

  it('no-ops when not running in Tauri (browser / PWA get the real web push)', async () => {
    isTauri.mockReturnValue(false);
    emit();
    await flush();
    expect(showNativeNotification).not.toHaveBeenCalled();
  });

  it("skips a banner meant for other devices (a root thread's audience)", async () => {
    localStorage.setItem(DEVICE_ID_KEY, 'this-mac');
    emit({ audience: ['someone-elses-phone'] });
    await flush();
    expect(showNativeNotification).not.toHaveBeenCalled();
    emit({ audience: ['this-mac'] });
    await flush();
    expect(showNativeNotification).toHaveBeenCalledTimes(1);
  });

  it('drops a stale frame (late SSE-queue flush) past the freshness budget', async () => {
    emit({ sent_at_ms: Date.now() - NATIVE_PUSH_STALE_AFTER_MS - 1 });
    await flush();
    expect(showNativeNotification).not.toHaveBeenCalled();
  });

  it('gives a window that came forward the in-app outcome instead of a banner', async () => {
    presentInAppNotification.mockReturnValue('shown');
    emit({ notification_id: 'n-front', thread_id: 't-1', event_id: 'e-1', tap: { kind: 'modal' } });
    await flush();
    expect(presentInAppNotification).toHaveBeenCalledWith({
      title: 'Claude is asking',
      body: 'Pick one',
      target: { notification: 'n-front', thread: 't-1', event: 'e-1', tap: { kind: 'modal' } },
    });
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
    takePendingNativeTaps.mockResolvedValue([]);
  });

  it('registers a Tauri listener and drains the stash (not the event payload)', async () => {
    await setupNativePushTapRouting();
    expect(listen).toHaveBeenCalledWith('native-notification-tapped', expect.any(Function));
    expect(listenHandler).toBeTruthy();
  });

  it('routes a tap stashed BEFORE the listener existed (cold / startup drain)', async () => {
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
    await setupNativePushTapRouting();
    await flush();
    expect(dispatchDeepLink).not.toHaveBeenCalled();

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

  it('drains and routes when the window regains focus (no-reload window-show path)', async () => {
    await setupNativePushTapRouting();
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
    await setupNativePushTapRouting();
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

  it('runs the startup drain only after the live listener is wired', async () => {
    let listenerWiredAtDrain = false;
    takePendingNativeTaps.mockImplementationOnce(() => {
      listenerWiredAtDrain = listenHandler !== null;
      return Promise.resolve([]);
    });
    await setupNativePushTapRouting();
    await flush();
    expect(listenerWiredAtDrain).toBe(true);
  });

  it('takes the tap from the stash, never from the live signal payload', async () => {
    await setupNativePushTapRouting();
    await flush();
    dispatchDeepLink.mockClear();

    takePendingNativeTaps.mockResolvedValueOnce([
      { notification_id: 'n-stashed', tap: { kind: 'modal' } },
    ]);
    listenHandler!({ payload: { notification_id: 'n-from-payload', tap: { kind: 'modal' } } });
    await flush();
    expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
    expect(dispatchDeepLink).toHaveBeenCalledWith(
      expect.objectContaining({ notification: 'n-stashed' }),
    );
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

function emitDismiss(overrides: Partial<NativePushDismissRequestedPayload> = {}): void {
  handleNativePushDismiss({
    notification_id:
      'notification_id' in overrides ? (overrides.notification_id as string | null) : 'notif-1',
    sent_at_ms: overrides.sent_at_ms ?? Date.now(),
    audience: overrides.audience ?? null,
  });
}

describe('NativePushDismissRequested → remove native desktop banner', () => {
  beforeEach(() => {
    isTauri.mockReturnValue(true);
    presentInAppNotification.mockClear();
    dismissNativeNotification.mockClear();
  });

  it('removes one banner by id, naming the workspace that raised it', async () => {
    emitDismiss({ notification_id: 'n-7' });
    await flush();
    expect(dismissNativeNotification).toHaveBeenCalledWith({
      workspace: 'myws',
      notificationId: 'n-7',
    });
  });

  it("keeps a banner when another person read it (a team gateway)", async () => {
    localStorage.setItem(DEVICE_ID_KEY, 'this-mac');
    emitDismiss({ audience: ['someone-elses-phone'] });
    await flush();
    expect(dismissNativeNotification).not.toHaveBeenCalled();
    emitDismiss({ audience: ['this-mac', 'my-phone'] });
    await flush();
    expect(dismissNativeNotification).toHaveBeenCalledTimes(1);
  });

  it('removes a banner while the window is in front (no active gate, unlike show)', async () => {
    const prev = document.visibilityState;
    try {
      (document as unknown as { visibilityState: string }).visibilityState = 'visible';
      emitDismiss({ notification_id: 'n-front' });
      await flush();
      expect(dismissNativeNotification).toHaveBeenCalledWith({
        workspace: 'myws',
        notificationId: 'n-front',
      });
    } finally {
      (document as unknown as { visibilityState: string }).visibilityState = prev;
    }
  });

  it('scopes a mark-all-read dismiss to this workspace', async () => {
    emitDismiss({ notification_id: null });
    await flush();
    expect(dismissNativeNotification).toHaveBeenCalledWith({
      workspace: 'myws',
      notificationId: null,
    });
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

  it('dismisses without asking whether the window is in front', async () => {
    emitDismiss({ notification_id: 'n-9' });
    await flush();
    expect(presentInAppNotification).not.toHaveBeenCalled();
    expect(dismissNativeNotification).toHaveBeenCalledWith({
      workspace: 'myws',
      notificationId: 'n-9',
    });
  });
});

describe('drain → workspace scoping', () => {
  const assign = vi.fn();
  let originalLocation: Location;

  beforeEach(() => {
    isTauri.mockReturnValue(true);
    listenHandler = null;
    dispatchDeepLink.mockClear();
    assign.mockClear();
    takePendingNativeTaps.mockClear();
    takePendingNativeTaps.mockResolvedValue([]);
    workspaceId = 'myws';
    originalLocation = window.location;
    Object.defineProperty(window, 'location', {
      value: { origin: 'https://localhost:5251', assign },
      configurable: true,
    });
  });

  afterEach(() => {
    Object.defineProperty(window, 'location', {
      value: originalLocation,
      configurable: true,
    });
  });

  it('drains with THIS page\'s workspace, which is what scopes it', async () => {
    await setupNativePushTapRouting();
    await flush();
    expect(takePendingNativeTaps).toHaveBeenCalledWith('myws');
  });

  it('re-reads the workspace on every drain, not once at module load', async () => {
    await setupNativePushTapRouting();
    await flush();
    workspaceId = 'otherws';
    window.dispatchEvent(new Event('focus'));
    await flush();
    expect(takePendingNativeTaps).toHaveBeenLastCalledWith('otherws');
  });

  it('passes null on a legacy engine with no gateway', async () => {
    workspaceId = null;
    await setupNativePushTapRouting();
    await flush();
    expect(takePendingNativeTaps).toHaveBeenCalledWith(null);
  });

  it('dispatches what it is handed and never navigates the window', async () => {
    takePendingNativeTaps.mockResolvedValueOnce([
      { notification_id: 'n-mine', workspace: 'myws', tap: { kind: 'modal' } },
      {
        notification_id: 'n-also-mine',
        workspace: 'myws',
        tap: { kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker' } },
      },
    ]);
    await setupNativePushTapRouting();
    await flush();
    expect(dispatchDeepLink).toHaveBeenCalledTimes(2);
    expect(dispatchDeepLink).toHaveBeenCalledWith(
      expect.objectContaining({ notification: 'n-mine' }),
    );
    expect(dispatchDeepLink).toHaveBeenCalledWith(
      expect.objectContaining({ notification: 'n-also-mine' }),
    );
    expect(assign).not.toHaveBeenCalled();
  });

  it('skips a stashed entry that is not an object at all', async () => {
    takePendingNativeTaps.mockResolvedValueOnce([
      'nope' as unknown as Record<string, unknown>,
      { notification_id: 'n-ok', workspace: 'myws', tap: { kind: 'modal' } },
    ]);
    await setupNativePushTapRouting();
    await flush();
    expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
    expect(dispatchDeepLink).toHaveBeenCalledWith(
      expect.objectContaining({ notification: 'n-ok' }),
    );
  });
});
