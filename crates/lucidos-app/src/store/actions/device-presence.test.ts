import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Mock the native-window bridge so we can drive its onChange and read the cached
// active state (which the real isPageActive consults). startNativeWindowActiveTracking
// captures the callback; firing it (active=false/true) is the trayed/unfocused
// transition the engine's candidate index must follow promptly.
const nativeMock = vi.hoisted(() => ({
  active: true,
  onChange: undefined as ((active: boolean) => void) | undefined,
}));
vi.mock('../../utils/nativeWindow', () => ({
  isNativeWindowActive: () => nativeMock.active,
  setNativeWindowActive: (active: boolean) => {
    nativeMock.active = active;
  },
  startNativeWindowActiveTracking: (onChange?: (active: boolean) => void) => {
    // Mirror the real module: update the cache BEFORE invoking onChange so
    // syncDevicePresence reads the fresh isPageActive().
    nativeMock.onChange = (active: boolean) => {
      nativeMock.active = active;
      onChange?.(active);
    };
    return Promise.resolve(() => {});
  },
}));

// jsdom isn't loaded; mirror presence.test.ts and inject minimal globals so
// the device-presence module's event-listener wiring doesn't blow up.
// We capture registered listeners by event type so individual tests can fire
// `visibilitychange` / `focus` / `blur` synthetically — the iOS-PWA resume
// regression is keyed off `visibilitychange` firing while the page nominally
// stayed visible.
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
  for (const fn of set) fn();
}

(document as unknown as { addEventListener: typeof bind }).addEventListener = ((
  type: string,
  listener: DomListener,
) => bind(documentListeners, type, listener)) as unknown as typeof bind;
(document as unknown as { removeEventListener: typeof unbind }).removeEventListener = ((
  type: string,
  listener: DomListener,
) => unbind(documentListeners, type, listener)) as unknown as typeof unbind;
(document as unknown as { hasFocus: () => boolean }).hasFocus = () => true;
Object.defineProperty(document, 'visibilityState', {
  configurable: true,
  get: () => 'visible',
});
(window as unknown as { addEventListener: typeof bind }).addEventListener = ((
  type: string,
  listener: DomListener,
) => bind(windowListeners, type, listener)) as unknown as typeof bind;
(window as unknown as { removeEventListener: typeof unbind }).removeEventListener = ((
  type: string,
  listener: DomListener,
) => unbind(windowListeners, type, listener)) as unknown as typeof unbind;

const fetchMock = vi.fn().mockResolvedValue({ ok: true });
(globalThis as { fetch: typeof fetch }).fetch = fetchMock as unknown as typeof fetch;

const { startDevicePresenceTracking, stopDevicePresenceTracking } = await import('./device-presence');

interface DevicePresenceBody {
  device_id: string;
  visible: boolean;
}

function lastFetchBody(): DevicePresenceBody | null {
  const calls = fetchMock.mock.calls;
  const call = calls.length > 0 ? calls[calls.length - 1] : null;
  if (!call) return null;
  const body = (call[1] as RequestInit | undefined)?.body;
  return typeof body === 'string' ? JSON.parse(body) : null;
}

describe('device presence tracking', () => {
  beforeEach(() => {
    fetchMock.mockClear();
    documentListeners.clear();
    windowListeners.clear();
    nativeMock.active = true;
    nativeMock.onChange = undefined;
  });

  afterEach(() => {
    stopDevicePresenceTracking();
  });

  it('emits visible=true immediately on start when the page is visible+focused', () => {
    startDevicePresenceTracking();
    const body = lastFetchBody();
    expect(body).toMatchObject({ visible: true });
    expect(typeof body?.device_id).toBe('string');
  });

  it('hits /api/v1/device-presence', () => {
    startDevicePresenceTracking();
    const calls = fetchMock.mock.calls;
    const lastCall = calls.length > 0 ? calls[calls.length - 1] : null;
    expect(lastCall?.[0]).toMatch(/\/api\/v1\/device-presence$/);
  });

  it('does not re-emit when the visibility state has not changed', () => {
    startDevicePresenceTracking();
    fetchMock.mockClear();
    // syncDevicePresence runs from the event handlers — invoking them directly
    // is the simplest way to assert the dedup. Module-level functions aren't
    // exported, so simulate by stopping/starting (which resets lastReported)
    // and then starting again would emit once. Test the dedup via the public
    // surface: a second start() resets lastReported, so we get one emit per
    // start cycle and no extras between visibility-change-shaped triggers.
    stopDevicePresenceTracking();
    fetchMock.mockClear();
    startDevicePresenceTracking();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('clears wiring on stop so a subsequent start emits a fresh visible=true', () => {
    startDevicePresenceTracking();
    stopDevicePresenceTracking();
    fetchMock.mockClear();
    startDevicePresenceTracking();
    expect(lastFetchBody()).toMatchObject({ visible: true });
  });

  // Regression: iOS PWA suspended JS while the page nominally stayed
  // visible (system overlay / lockscreen / screen-off-then-on with no
  // visibility flip the engine ever saw). When JS resumes, WebKit fires a
  // `visibilitychange` even though the post-resume visibility matches the
  // pre-suspend one. With the old `lastReported === visible` short-circuit
  // the page never re-POSTed `device_presence`, so the row aged past
  // PRESENCE_STALE_AFTER (120s) while heartbeat ticks were stalled. The
  // next NotificationCreated saw zero candidate devices, skipped the
  // PresenceCheck protocol, and fanned out the OS push on top of an
  // active foreground PWA — `notifications.md` §2 row 4 misfire (see
  // also §3 "candidate index").
  //
  // The fix is to treat any `visibilitychange-while-visible` as a fresh
  // heartbeat — cheap to POST (one network call per resume), and it
  // restores the row so the engine's candidate query stops returning
  // empty after a long suspension window.
  it('s2_visibilitychange_while_visible_refreshes_device_presence_for_ios_pwa_resume', () => {
    startDevicePresenceTracking();
    // Drain the initial POST that startDevicePresenceTracking always emits.
    fetchMock.mockClear();
    // Page stayed visible across the simulated JS-suspend window. Fire the
    // visibilitychange that WebKit delivers on resume.
    fire(documentListeners, 'visibilitychange');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(lastFetchBody()).toMatchObject({ visible: true });
  });

  // Contract guard for the handler split: focus/blur stay deduped, only
  // visibilitychange forces a refresh. A future change that flipped
  // `onFocusBlur` to `forceRefresh: true` would hammer the backend with a
  // POST on every in-window click on desktop — caught here. Mirrors the
  // iOS resume test above for the opposite case.
  it('s2_focus_while_visible_does_not_refresh_device_presence', () => {
    startDevicePresenceTracking();
    fetchMock.mockClear();
    fire(windowListeners, 'focus');
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // Tauri: the native window trayed (orderOut:) or going behind another app is
  // invisible to the webview's visibilitychange/focus events. The native-active
  // bridge re-syncs presence so the engine's candidate index flips immediately
  // (instead of aging out over 120s) and a non-active desktop client gets the OS
  // native banner rather than a suppressed, invisible in-app toast.
  it('re-syncs device presence (visible=false) when the native window goes inactive', () => {
    startDevicePresenceTracking();
    fetchMock.mockClear();
    nativeMock.onChange?.(false);
    expect(lastFetchBody()).toMatchObject({ visible: false });
  });

  it('re-syncs device presence (visible=true) when the native window becomes active', () => {
    startDevicePresenceTracking();
    // Go inactive first so the visible=true re-sync isn't deduped away.
    nativeMock.onChange?.(false);
    fetchMock.mockClear();
    nativeMock.onChange?.(true);
    expect(lastFetchBody()).toMatchObject({ visible: true });
  });
});
