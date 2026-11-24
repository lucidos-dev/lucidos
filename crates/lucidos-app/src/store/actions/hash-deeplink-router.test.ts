import { describe, it, expect, beforeEach, vi } from 'vitest';

type DomListener = (ev?: unknown) => void;
const documentListeners = new Map<string, Set<DomListener>>();
const windowListeners = new Map<string, Set<DomListener>>();

function bind(map: Map<string, Set<DomListener>>, type: string, listener: DomListener) {
  let set = map.get(type);
  if (!set) {
    set = new Set();
    map.set(type, set);
  }
  set.add(listener);
}
function unbind(map: Map<string, Set<DomListener>>, type: string, listener: DomListener) {
  map.get(type)?.delete(listener);
}
function fire(map: Map<string, Set<DomListener>>, type: string) {
  const set = map.get(type);
  if (!set) return;
  for (const fn of [...set]) fn();
}

(document as unknown as { addEventListener: typeof bind }).addEventListener = ((
  type: string,
  listener: DomListener,
) => bind(documentListeners, type, listener)) as unknown as typeof bind;
(document as unknown as { removeEventListener: typeof unbind }).removeEventListener = ((
  type: string,
  listener: DomListener,
) => unbind(documentListeners, type, listener)) as unknown as typeof unbind;
let visibilityValue: 'visible' | 'hidden' = 'visible';
Object.defineProperty(document, 'visibilityState', {
  configurable: true,
  get: () => visibilityValue,
});
(window as unknown as { addEventListener: typeof bind }).addEventListener = ((
  type: string,
  listener: DomListener,
) => bind(windowListeners, type, listener)) as unknown as typeof bind;
(window as unknown as { removeEventListener: typeof unbind }).removeEventListener = ((
  type: string,
  listener: DomListener,
) => unbind(windowListeners, type, listener)) as unknown as typeof unbind;

const currentUrl = { value: 'http://localhost/' };
function parsed() {
  return new URL(currentUrl.value);
}
Object.defineProperty(window, 'location', {
  configurable: true,
  get() {
    const u = parsed();
    return {
      href: u.href,
      hash: u.hash,
      pathname: u.pathname,
      search: u.search,
      origin: u.origin,
    };
  },
});
Object.defineProperty(window, 'history', {
  configurable: true,
  value: { replaceState: (_state: unknown, _title: string, url: string) => { currentUrl.value = new URL(url, currentUrl.value).href; } },
});
function setUrl(href: string) {
  currentUrl.value = href;
}

type Outcome = { kind: 'focused' } | { kind: 'not-found' } | { kind: 'failed'; error: unknown };
const focusThreadOrBootstrapResult = vi.fn<(id: string) => Promise<Outcome>>();
const dispatchDeepLink = vi.fn();
const showToast = vi.fn();
const switchMenuItem = vi.fn();

vi.mock('./threads', () => ({
  focusThreadOrBootstrapResult: (...args: [string]) => focusThreadOrBootstrapResult(...args),
}));
vi.mock('./menu', async () => {
  const actual = await vi.importActual<typeof import('./menu')>('./menu');
  return { ...actual, switchMenuItem: (...args: unknown[]) => switchMenuItem(...args) };
});
vi.mock('./in-app-notification-toast', () => ({ dispatchDeepLink: (...args: unknown[]) => dispatchDeepLink(...args) }));
const INSTALLED_APP = 'habit-tracker';
const openAppById = vi.fn((id: string, _source?: string, _fragment?: string) =>
  Promise.resolve(id === INSTALLED_APP));
vi.mock('./apps', async () => {
  const actual = await vi.importActual<typeof import('./apps')>('./apps');
  return { ...actual, openAppById: (...args: [string, string?, string?]) => openAppById(...args) };
});
const replaceNavState = vi.fn();
vi.mock('./navigation', async () => {
  const actual = await vi.importActual<typeof import('./navigation')>('./navigation');
  return { ...actual, replaceNavState: () => replaceNavState() };
});
vi.mock('../store', async () => {
  const actual = await vi.importActual<typeof import('../store')>('../store');
  return { ...actual, showToast: (...args: unknown[]) => showToast(...args) };
});

const { handleHashLocation, setupHashDeeplinkRouting, _resetThreadHashLandingForTesting } =
  await import('./hash-deeplink-router');
const { landingHash } = await import('../../utils/workspaceLanding');
const { appLandingHash } = await import('./appHash');
const { appPseudoFullscreen, focusedThreadId, threadsLoaded, wipPreviewThreadId } = await import('../store');

const flush = () => new Promise<void>(resolve => { setTimeout(resolve, 0); });

const NOTIF_ID = '5d8c4d96-8df1-4243-a43d-35449f689bda';
const THREAD_ID = '80012b3a-b3cf-4a89-bd23-a35363db4177';
const EVENT_ID = '0904bab6-4358-4dab-bf48-f3d89cc68a08';

function buildNavigateHash(): string {
  const tap = encodeURIComponent(
    JSON.stringify({ kind: 'navigate', to: { target: 'thread', id: THREAD_ID, event_id: EVENT_ID } }),
  );
  return `#notification=${NOTIF_ID}&thread=${THREAD_ID}&event=${EVENT_ID}&tap=${tap}`;
}

function buildNavigateQuery(): string {
  const tap = encodeURIComponent(
    JSON.stringify({ kind: 'navigate', to: { target: 'thread', id: THREAD_ID, event_id: EVENT_ID } }),
  );
  return `?notification=${NOTIF_ID}&thread=${THREAD_ID}&event=${EVENT_ID}&tap=${tap}`;
}

describe('hash-deeplink-router', () => {
  beforeEach(() => {
    focusThreadOrBootstrapResult.mockReset();
    focusThreadOrBootstrapResult.mockResolvedValue({ kind: 'focused' });
    dispatchDeepLink.mockClear();
    showToast.mockClear();
    switchMenuItem.mockClear();
    openAppById.mockClear();
    replaceNavState.mockClear();
    appPseudoFullscreen.value = false;
    wipPreviewThreadId.value = null;
    focusedThreadId.value = null;
    documentListeners.clear();
    windowListeners.clear();
    visibilityValue = 'visible';
    setUrl('http://localhost/');
    _resetThreadHashLandingForTesting();
  });

  describe('the bare #notifications landing channel', () => {
    it('lands on the notifications view and consumes the hash', () => {
      setUrl('http://localhost/dev/#notifications');
      handleHashLocation();
      expect(switchMenuItem).toHaveBeenCalledWith('notifications');
      expect(currentUrl.value, 'the consumed hash must be stripped')
        .toBe('http://localhost/dev/');
      expect(dispatchDeepLink).not.toHaveBeenCalled();
    });

    it('does not re-fire on a resume once consumed', () => {
      setUrl('http://localhost/dev/#notifications');
      handleHashLocation();
      handleHashLocation();
      expect(switchMenuItem).toHaveBeenCalledTimes(1);
    });

    it('matches the fragment the opener composes', () => {
      setUrl(`http://localhost/dev/${landingHash('notifications')}`);
      handleHashLocation();
      expect(switchMenuItem).toHaveBeenCalledWith('notifications');
    });

    it('does NOT swallow the push-tap channel, whose key it is a prefix of', () => {
      setUrl(`http://localhost/${buildNavigateHash()}`);
      handleHashLocation();
      expect(switchMenuItem).not.toHaveBeenCalled();
      expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
    });

    it('does NOT swallow the cross-workspace thread channel', () => {
      setUrl(`http://localhost/#thread=${THREAD_ID}`);
      handleHashLocation();
      expect(switchMenuItem).not.toHaveBeenCalled();
      expect(focusThreadOrBootstrapResult).toHaveBeenCalledWith(THREAD_ID);
    });

    it('ignores a hash that merely starts with the word', () => {
      setUrl('http://localhost/#notifications-and-more');
      handleHashLocation();
      expect(switchMenuItem).not.toHaveBeenCalled();
    });
  });

  describe('the #app= popout landing channel', () => {
    it('opens the installed app in pseudo-fullscreen and consumes the hash', async () => {
      setUrl(`http://localhost/dev@peer/${appLandingHash({ appId: INSTALLED_APP })}`);
      handleHashLocation();
      await flush();
      expect(openAppById).toHaveBeenCalledWith(INSTALLED_APP, 'a popout link', undefined);
      expect(appPseudoFullscreen.value).toBe(true);
      expect(currentUrl.value, 'the consumed hash must be stripped')
        .toBe('http://localhost/dev@peer/');
    });

    it('shows the live app, even when the seeded history restored a WIP preview', async () => {
      focusedThreadId.value = 'thread-7';
      threadsLoaded.value = false;
      wipPreviewThreadId.value = 'thread-7';
      setUrl(`http://localhost/dev@peer/${appLandingHash({ appId: INSTALLED_APP })}`);
      handleHashLocation();
      await flush();
      expect(wipPreviewThreadId.value).toBeNull();
      expect(replaceNavState, 'the history row must match the live app').toHaveBeenCalledTimes(1);
    });

    it('carries the app fragment through, escapes and all', async () => {
      const fragment = 'week=3&view=list';
      setUrl(`http://localhost/dev@peer/${appLandingHash({ appId: INSTALLED_APP, fragment })}`);
      handleHashLocation();
      await flush();
      expect(openAppById).toHaveBeenCalledWith(INSTALLED_APP, 'a popout link', fragment);
    });

    it('opens nothing and stays in the normal layout for an unknown id', async () => {
      setUrl(`http://localhost/dev@peer/${appLandingHash({ appId: 'not-installed' })}`);
      handleHashLocation();
      await flush();
      expect(openAppById).toHaveBeenCalledWith('not-installed', 'a popout link', undefined);
      expect(appPseudoFullscreen.value).toBe(false);
      expect(currentUrl.value).toBe('http://localhost/dev@peer/');
    });

    it('does not re-fire on a resume once consumed', async () => {
      setUrl(`http://localhost/dev@peer/${appLandingHash({ appId: INSTALLED_APP })}`);
      handleHashLocation();
      handleHashLocation();
      await flush();
      expect(openAppById).toHaveBeenCalledTimes(1);
    });

    it('swallows none of the other channels', () => {
      for (const hash of ['#notifications', `#thread=${THREAD_ID}`, buildNavigateHash(), '#apps', '#application=x']) {
        setUrl(`http://localhost/${hash}`);
        _resetThreadHashLandingForTesting();
        handleHashLocation();
      }
      expect(openAppById).not.toHaveBeenCalled();
    });
  });

  describe('handleHashLocation', () => {
    it('s4_5_handle_hash_location_dispatches_notification_navigate_target', () => {
      setUrl(`http://localhost/${buildNavigateHash()}`);
      handleHashLocation();
      expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
      const target = dispatchDeepLink.mock.calls[0][0] as Record<string, unknown>;
      expect(target.notification).toBe(NOTIF_ID);
      expect(target.thread).toBe(THREAD_ID);
      expect(target.event).toBe(EVENT_ID);
    });

    it('s4_5_handle_hash_location_dispatches_query_url_for_ios_declarative_reload', () => {
      setUrl(`http://localhost/${buildNavigateQuery()}`);
      handleHashLocation();
      expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
      const target = dispatchDeepLink.mock.calls[0][0] as Record<string, unknown>;
      expect(target.notification).toBe(NOTIF_ID);
      expect(target.thread).toBe(THREAD_ID);
      expect(target.event).toBe(EVENT_ID);
      expect(window.location.search).toBe('');
    });

    it('s4_5_handle_hash_location_strips_deep_link_hash_after_dispatch_idempotent', () => {
      setUrl(`http://localhost/${buildNavigateHash()}`);
      handleHashLocation();
      expect(window.location.hash).toBe('');
      handleHashLocation();
      expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
    });

    it('s4_5_handle_hash_location_routes_bare_thread_hash_via_focus_thread_or_bootstrap', async () => {
      setUrl(`http://localhost/#thread=${THREAD_ID}`);
      handleHashLocation();
      expect(focusThreadOrBootstrapResult).toHaveBeenCalledTimes(1);
      expect(focusThreadOrBootstrapResult.mock.calls[0][0]).toBe(THREAD_ID);
      await flush();
      expect(window.location.hash).toBe('');
      expect(dispatchDeepLink).not.toHaveBeenCalled();
    });

    it('s4_5_handle_hash_location_noops_on_unrecognized_hash_anchor', () => {
      setUrl('http://localhost/#some-other-anchor');
      handleHashLocation();
      expect(dispatchDeepLink).not.toHaveBeenCalled();
      expect(focusThreadOrBootstrapResult).not.toHaveBeenCalled();
      expect(window.location.hash).toBe('#some-other-anchor');
    });
  });

  describe('bare #thread= landing durability', () => {
    it('keeps the hash while a failed bootstrap is retried, and strips it on success', async () => {
      vi.useFakeTimers();
      try {
        focusThreadOrBootstrapResult
          .mockResolvedValueOnce({ kind: 'failed', error: new Error('engine booting') })
          .mockResolvedValueOnce({ kind: 'failed', error: new Error('engine booting') })
          .mockResolvedValue({ kind: 'focused' });

        setUrl(`http://localhost/#thread=${THREAD_ID}`);
        handleHashLocation();

        await vi.advanceTimersByTimeAsync(0);
        expect(focusThreadOrBootstrapResult).toHaveBeenCalledTimes(1);
        expect(window.location.hash).toBe(`#thread=${THREAD_ID}`);

        await vi.advanceTimersByTimeAsync(5000);
        expect(focusThreadOrBootstrapResult).toHaveBeenCalledTimes(3);
        expect(window.location.hash).toBe('');
        expect(showToast).not.toHaveBeenCalled();
      } finally {
        vi.useRealTimers();
      }
    });

    it('does not retry a not-found verdict, and tells the user which thread', async () => {
      focusThreadOrBootstrapResult.mockResolvedValue({ kind: 'not-found' });
      setUrl(`http://localhost/#thread=${THREAD_ID}`);
      handleHashLocation();
      await flush();
      expect(focusThreadOrBootstrapResult).toHaveBeenCalledTimes(1);
      expect(window.location.hash).toBe('');
      expect(showToast).toHaveBeenCalledTimes(1);
      expect(showToast.mock.calls[0][0]).toContain(THREAD_ID);
      expect(showToast.mock.calls[0][1]).toBe('error');
    });

    it('gives up after a bounded number of attempts and surfaces the error', async () => {
      vi.useFakeTimers();
      try {
        focusThreadOrBootstrapResult.mockResolvedValue({
          kind: 'failed',
          error: new Error('peer unreachable'),
        });
        setUrl(`http://localhost/#thread=${THREAD_ID}`);
        handleHashLocation();

        await vi.advanceTimersByTimeAsync(60_000);
        expect(focusThreadOrBootstrapResult.mock.calls.length).toBeLessThan(20);
        expect(window.location.hash).toBe('');
        expect(showToast).toHaveBeenCalledTimes(1);
        expect(showToast.mock.calls[0][0]).toContain('peer unreachable');
      } finally {
        vi.useRealTimers();
      }
    });

    it('does not start a second landing while one is in flight for the same thread', async () => {
      vi.useFakeTimers();
      try {
        focusThreadOrBootstrapResult.mockResolvedValue({
          kind: 'failed',
          error: new Error('engine booting'),
        });
        setUrl(`http://localhost/#thread=${THREAD_ID}`);
        handleHashLocation();
        await vi.advanceTimersByTimeAsync(0);
        expect(focusThreadOrBootstrapResult).toHaveBeenCalledTimes(1);

        handleHashLocation();
        handleHashLocation();
        expect(focusThreadOrBootstrapResult).toHaveBeenCalledTimes(1);
      } finally {
        vi.useRealTimers();
      }
    });
  });

  describe('setupHashDeeplinkRouting (iOS PWA resume — the regression)', () => {
    it('s4_5_setup_routing_runs_handle_hash_location_on_visibilitychange_visible_for_ios_pwa_resume', () => {
      const teardown = setupHashDeeplinkRouting();
      try {
        setUrl(`http://localhost/${buildNavigateHash()}`);
        expect(dispatchDeepLink).not.toHaveBeenCalled();

        fire(documentListeners, 'visibilitychange');

        expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
        expect(window.location.hash).toBe('');
      } finally {
        teardown();
      }
    });

    it('s4_5_setup_routing_skips_dispatch_when_document_is_hidden', () => {
      const teardown = setupHashDeeplinkRouting();
      try {
        setUrl(`http://localhost/${buildNavigateHash()}`);
        visibilityValue = 'hidden';
        fire(documentListeners, 'visibilitychange');
        fire(windowListeners, 'focus');
        fire(windowListeners, 'pageshow');
        expect(dispatchDeepLink).not.toHaveBeenCalled();
      } finally {
        teardown();
      }
    });

    it('s4_5_setup_routing_runs_handle_hash_location_on_window_focus', () => {
      const teardown = setupHashDeeplinkRouting();
      try {
        setUrl(`http://localhost/${buildNavigateHash()}`);
        fire(windowListeners, 'focus');
        expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
      } finally {
        teardown();
      }
    });

    it('s4_5_setup_routing_runs_handle_hash_location_on_pageshow_bfcache_restore', () => {
      const teardown = setupHashDeeplinkRouting();
      try {
        setUrl(`http://localhost/${buildNavigateHash()}`);
        fire(windowListeners, 'pageshow');
        expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
      } finally {
        teardown();
      }
    });

    it('s4_5_setup_routing_runs_handle_hash_location_on_hashchange_warm_path', () => {
      const teardown = setupHashDeeplinkRouting();
      try {
        setUrl(`http://localhost/${buildNavigateHash()}`);
        fire(windowListeners, 'hashchange');
        expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
      } finally {
        teardown();
      }
    });

    it('s4_5_setup_routing_teardown_removes_every_registered_listener', () => {
      const teardown = setupHashDeeplinkRouting();
      teardown();
      setUrl(`http://localhost/${buildNavigateHash()}`);
      fire(documentListeners, 'visibilitychange');
      fire(windowListeners, 'focus');
      fire(windowListeners, 'pageshow');
      fire(windowListeners, 'hashchange');
      expect(dispatchDeepLink).not.toHaveBeenCalled();
    });

    it('s4_5_setup_routing_teardown_clears_cold_start_timer', () => {
      vi.useFakeTimers();
      try {
        const teardown = setupHashDeeplinkRouting();
        setUrl(`http://localhost/${buildNavigateHash()}`);
        teardown();
        vi.advanceTimersByTime(1000);
        expect(dispatchDeepLink).not.toHaveBeenCalled();
      } finally {
        vi.useRealTimers();
      }
    });
  });
});
