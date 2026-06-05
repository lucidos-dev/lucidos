import { describe, it, expect, beforeEach, vi } from 'vitest';

// Mirror device-presence.test.ts: jsdom isn't loaded; inject minimal globals
// so the module's event-listener wiring doesn't blow up. We capture every
// registered listener by event type so individual tests can fire
// `visibilitychange` / `focus` / `pageshow` / `hashchange` synthetically.
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

// Vitest runs in Node (no jsdom). Fake window.location / window.history with
// just enough surface for the router: `hash` / `pathname` / `search` /
// `href` reads and `history.replaceState` writes. Crucially, replaceState
// updates the URL but does NOT fire hashchange — that's exactly the iOS PWA
// resume scenario (Safari updated the URL while JS was suspended; the paired
// hashchange never reached the page).
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
// URL constructor needs polyfilling? It's available in Node natively.

// Mock the two dispatchers the router calls. Replaces the real focus/dispatch
// side-effect chain with vi.fn so the test owns assertion of "did it fire?".
const focusThreadOrBootstrap = vi.fn();
const dispatchDeepLink = vi.fn();

vi.mock('./threads', () => ({ focusThreadOrBootstrap: (...args: unknown[]) => focusThreadOrBootstrap(...args) }));
vi.mock('./in-app-notification-toast', () => ({ dispatchDeepLink: (...args: unknown[]) => dispatchDeepLink(...args) }));

const { handleHashLocation, setupHashDeeplinkRouting } = await import('./hash-deeplink-router');

const NOTIF_ID = '5d8c4d96-8df1-4243-a43d-35449f689bda';
const THREAD_ID = '80012b3a-b3cf-4a89-bd23-a35363db4177';
const EVENT_ID = '0904bab6-4358-4dab-bf48-f3d89cc68a08';

function buildNavigateHash(): string {
  const tap = encodeURIComponent(
    JSON.stringify({ kind: 'navigate', to: { target: 'thread', id: THREAD_ID, event_id: EVENT_ID } }),
  );
  return `#notification=${NOTIF_ID}&thread=${THREAD_ID}&event=${EVENT_ID}&tap=${tap}`;
}

/** The QUERY (cross-document) form of the same deep link. iOS Safari's
 *  declarative push navigates an already-open PWA window to this URL (a hash-
 *  only change would just focus the window without navigating), so the page
 *  lands on it via a real load/resume and must route off the query string. */
function buildNavigateQuery(): string {
  const tap = encodeURIComponent(
    JSON.stringify({ kind: 'navigate', to: { target: 'thread', id: THREAD_ID, event_id: EVENT_ID } }),
  );
  return `?notification=${NOTIF_ID}&thread=${THREAD_ID}&event=${EVENT_ID}&tap=${tap}`;
}

describe('hash-deeplink-router', () => {
  beforeEach(() => {
    focusThreadOrBootstrap.mockClear();
    dispatchDeepLink.mockClear();
    documentListeners.clear();
    windowListeners.clear();
    visibilityValue = 'visible';
    setUrl('http://localhost/');
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
      // iOS Safari declarative push navigates the PWA window to the QUERY
      // navigate URL (cross-document, so an already-open window actually
      // navigates instead of just focusing). After the load/resume the router
      // must read the deep link from the query string, not just the hash.
      setUrl(`http://localhost/${buildNavigateQuery()}`);
      handleHashLocation();
      expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
      const target = dispatchDeepLink.mock.calls[0][0] as Record<string, unknown>;
      expect(target.notification).toBe(NOTIF_ID);
      expect(target.thread).toBe(THREAD_ID);
      expect(target.event).toBe(EVENT_ID);
      // Consumed query params are stripped so a refresh doesn't re-fire.
      expect(window.location.search).toBe('');
    });

    it('s4_5_handle_hash_location_strips_deep_link_hash_after_dispatch_idempotent', () => {
      setUrl(`http://localhost/${buildNavigateHash()}`);
      handleHashLocation();
      expect(window.location.hash).toBe('');
      handleHashLocation();
      // Still only one call total — second invocation found no deep-link.
      expect(dispatchDeepLink).toHaveBeenCalledTimes(1);
    });

    it('s4_5_handle_hash_location_routes_bare_thread_hash_via_focus_thread_or_bootstrap', () => {
      setUrl(`http://localhost/#thread=${THREAD_ID}`);
      handleHashLocation();
      expect(focusThreadOrBootstrap).toHaveBeenCalledTimes(1);
      expect(focusThreadOrBootstrap.mock.calls[0][0]).toBe(THREAD_ID);
      expect(window.location.hash).toBe('');
      expect(dispatchDeepLink).not.toHaveBeenCalled();
    });

    it('s4_5_handle_hash_location_noops_on_unrecognized_hash_anchor', () => {
      setUrl('http://localhost/#some-other-anchor');
      handleHashLocation();
      expect(dispatchDeepLink).not.toHaveBeenCalled();
      expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
      // Unrecognized anchor is preserved — only deep-link params are stripped.
      expect(window.location.hash).toBe('#some-other-anchor');
    });
  });

  describe('setupHashDeeplinkRouting (iOS PWA resume — the regression)', () => {
    it('s4_5_setup_routing_runs_handle_hash_location_on_visibilitychange_visible_for_ios_pwa_resume', () => {
      // The iOS PWA scenario: Safari handled the declarative push tap and
      // updated the PWA URL while the JS was suspended. When the page resumes,
      // hashchange does NOT fire (the event paired with the URL update was
      // delivered to a frozen runloop). Without this resume wiring, the
      // dispatcher never runs and the push tap silently no-ops.
      const teardown = setupHashDeeplinkRouting();
      try {
        // Simulate Safari setting the URL hash silently (no hashchange).
        // history.replaceState is the documented way to mutate the URL
        // without firing hashchange — same shape as iOS Safari's behavior.
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
        // Defense-in-depth: same gate applies to focus + pageshow when the
        // document is technically still hidden (e.g. DevTools focus, programmatic
        // focus on a background tab). Without this the deep link would dispatch
        // against a tab the user can't see and the hash would be stripped silently.
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
      // Fake timers so we can advance past the cold-start dispatch without
      // a real wall-clock wait. Without this assertion, a regression that
      // drops `clearTimeout(coldStartTimer)` from teardown would still pass
      // the listener-removal test above — the timer would fire on the next
      // macrotask (outside the synchronous test body) and silently dispatch.
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
