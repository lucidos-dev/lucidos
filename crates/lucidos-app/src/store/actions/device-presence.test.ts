import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

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
    nativeMock.onChange = (active: boolean) => {
      nativeMock.active = active;
      onChange?.(active);
    };
    return Promise.resolve(() => {});
  },
}));

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

const { observeSourceGeneration } = await import('../../utils/restoreSourceGeneration');
const { trackDeviceRegistration } = await import('../../utils/deviceRegistration');
const { installRestoreAdmission } = await import('../../utils/restoreAdmission');

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
    observeSourceGeneration(null);
    installRestoreAdmission({ ready: () => true, phase: () => 'admitted', wait: async () => {} });
    fetchMock.mockClear();
    documentListeners.clear();
    windowListeners.clear();
    nativeMock.active = true;
    nativeMock.onChange = undefined;
  });

  afterEach(() => {
    stopDevicePresenceTracking();
  });

  it('stamps the original generation and keeps unload delivery on the guarded transport', () => {
    startDevicePresenceTracking();
    expect(fetchMock.mock.calls[0][1].headers['x-lucidos-restore-generation']).toBe('none');
    fetchMock.mockClear();
    fire(windowListeners, 'beforeunload');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][1]).toMatchObject({ keepalive: true,
      headers: { 'x-lucidos-restore-generation': 'none' } });
    expect(lastFetchBody()).toMatchObject({ visible: false });
  });

  it('does not dispatch old presence when registration finishes after a restore', async () => {
    let release!: () => void;
    const registration = trackDeviceRegistration(new Promise<void>((resolve) => { release = resolve; }));
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      startDevicePresenceTracking();
      expect(fetchMock).not.toHaveBeenCalled();
      observeSourceGeneration('11111111-1111-4111-8111-111111111111');
      release();
      await registration;
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(fetchMock).not.toHaveBeenCalled();
    } finally { warning.mockRestore(); }
  });

  it('does not dispatch old presence when restore admission releases after a restore', async () => {
    let release!: () => void;
    let ready = false;
    const waiting = new Promise<void>((resolve) => { release = resolve; });
    installRestoreAdmission({ ready: () => ready, phase: () => ready ? 'admitted' : 'awaiting-hello', wait: () => waiting });
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      startDevicePresenceTracking();
      expect(fetchMock).not.toHaveBeenCalled();
      observeSourceGeneration('11111111-1111-4111-8111-111111111111');
      ready = true;
      release();
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(fetchMock).not.toHaveBeenCalled();
    } finally { warning.mockRestore(); }
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

  it('s2_visibilitychange_while_visible_refreshes_device_presence_for_ios_pwa_resume', () => {
    startDevicePresenceTracking();
    fetchMock.mockClear();
    fire(documentListeners, 'visibilitychange');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(lastFetchBody()).toMatchObject({ visible: true });
  });

  it('s2_focus_while_visible_does_not_refresh_device_presence', () => {
    startDevicePresenceTracking();
    fetchMock.mockClear();
    fire(windowListeners, 'focus');
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('re-syncs device presence (visible=false) when the native window goes inactive', () => {
    startDevicePresenceTracking();
    fetchMock.mockClear();
    nativeMock.onChange?.(false);
    expect(lastFetchBody()).toMatchObject({ visible: false });
  });

  it('re-syncs device presence (visible=true) when the native window becomes active', () => {
    startDevicePresenceTracking();
    nativeMock.onChange?.(false);
    fetchMock.mockClear();
    nativeMock.onChange?.(true);
    expect(lastFetchBody()).toMatchObject({ visible: true });
  });
});
