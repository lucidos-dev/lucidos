import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { panelOverlay, currentApp, appsList, appPseudoFullscreen, inputMode, appRefreshKey, splitRatio, toasts, wipPreviewThreadId, threadMap, focusedThreadId, threadsLoaded } from '../store';
import type { App } from '../types';
import { makeOptimisticThreadState } from '../thread-events';
import '../actions/wipPreview';

const mockPostAppCapture = vi.fn().mockResolvedValue(undefined);
const mockListAppsApi = vi.fn().mockResolvedValue([]);
const mockGetAppApi = vi.fn();
const mockRecordAppOpenedApi = vi.fn().mockResolvedValue(undefined);
vi.mock('../../api/client', async () => ({
  recordAppOpenedApi: (...args: unknown[]) => mockRecordAppOpenedApi(...args),
  ApiError: (await vi.importActual<typeof import('../../api/client/_core')>('../../api/client/_core')).ApiError,
  getAppApi: (...args: unknown[]) => mockGetAppApi(...args),
  postAppCapture: (...args: unknown[]) => mockPostAppCapture(...args),
  listAppsApi: (...args: unknown[]) => mockListAppsApi(...args),
  retryTransient: <T>(read: () => Promise<T>) => read(),
  appUrl: vi.fn((id: string, tid?: string, fragment?: string) =>
    `/app/${id}/${tid ? `?thread_id=${tid}` : ''}${fragment ? `#${fragment}` : ''}`),
}));

const mockPushNavState = vi.fn();
const mockReplaceNavState = vi.fn();
vi.mock('./navigation', () => ({
  pushNavState: (...args: unknown[]) => mockPushNavState(...args),
  replaceNavState: (...args: unknown[]) => mockReplaceNavState(...args),
}));

const notesApp: App = {
  id: 'notes-app',
  name: 'Notes App',
  description: 'Daily notes', reveal: 'on-load', kind: 'app', reusable: false,
};

const tripPlanner: App = {
  id: 'trip-planner-2026',
  name: 'Trip Planner 2026',
  description: 'Vacation planner', reveal: 'on-load', kind: 'app', reusable: false,
};

describe('captureAppUI', () => {
  let origQuerySelector: typeof document.querySelector;
  let origQuerySelectorAll: typeof document.querySelectorAll;

  beforeEach(() => {
    panelOverlay.value = null;
    inputMode.value = { type: 'do' };
    appsList.value = {
      status: 'loaded',
      data: [notesApp, tripPlanner],
    };
    mockPostAppCapture.mockClear();

    origQuerySelector = document.querySelector;
    origQuerySelectorAll = document.querySelectorAll;
    document.querySelector = vi.fn().mockReturnValue(null);
    document.querySelectorAll = vi.fn().mockReturnValue([]);
  });

  afterEach(() => {
    document.querySelector = origQuerySelector;
    document.querySelectorAll = origQuerySelectorAll;
  });

  it('does NOT auto-open a different app when no iframe exists (the no-iframe bug)', async () => {
    const { captureAppUI } = await import('./apps');
    await captureAppUI('notes-app', 'test-request-id');

    expect(panelOverlay.value).toBeNull();
    expect(currentApp.value).toBeNull();

    expect(mockPostAppCapture).toHaveBeenCalledWith(
      'test-request-id',
      '',
      expect.stringContaining('Error'),
    );
  });

  it('does NOT auto-open an app when user has a different panel open', async () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };

    const { captureAppUI } = await import('./apps');
    await captureAppUI('notes-app', 'test-request-id');

    expect(panelOverlay.value).toEqual({ type: 'file-preview', path: 'artifacts/notes.md' });

    expect(mockPostAppCapture).toHaveBeenCalledWith(
      'test-request-id',
      '',
      expect.stringContaining('Error'),
    );
  });

});

describe('refreshApps', () => {
  it('waits out a read already in flight, then sends its own', async () => {
    const { loadApps, refreshApps } = await import('./apps');
    let releaseFirst!: (apps: App[]) => void;
    mockListAppsApi.mockReset()
      .mockImplementationOnce(() => new Promise<App[]>((res) => { releaseFirst = res; }))
      .mockResolvedValue([notesApp, tripPlanner]);

    const first = loadApps();
    const refresh = refreshApps();
    await vi.waitFor(() => expect(mockListAppsApi).toHaveBeenCalledTimes(1));
    releaseFirst([notesApp]);
    await first;
    await refresh;

    expect(mockListAppsApi).toHaveBeenCalledTimes(2);
    expect(appsList.value).toEqual({ status: 'loaded', data: [notesApp, tripPlanner] });
  });
});

describe('refreshAppUI', () => {
  let origQuerySelectorAll: typeof document.querySelectorAll;

  beforeEach(() => {
    vi.useFakeTimers();
    panelOverlay.value = null;
    inputMode.value = { type: 'do' };
    appRefreshKey.value = 0;
    appsList.value = {
      status: 'loaded',
      data: [notesApp, tripPlanner],
    };
    mockListAppsApi.mockResolvedValue([notesApp, tripPlanner]);
    origQuerySelectorAll = document.querySelectorAll;
    document.querySelectorAll = vi.fn().mockReturnValue([]);
  });

  afterEach(() => {
    vi.useRealTimers();
    document.querySelectorAll = origQuerySelectorAll;
  });

  it('increments appRefreshKey when the target app is open', async () => {
    panelOverlay.value = { type: 'app-ui', app: notesApp };
    expect(appRefreshKey.value).toBe(0);

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI('notes-app');
    await vi.advanceTimersByTimeAsync(200);

    expect(appRefreshKey.value).toBe(1);
  });

  it('debounces multiple rapid calls into a single reload', async () => {
    panelOverlay.value = { type: 'app-ui', app: notesApp };

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI();
    await refreshAppUI();
    await refreshAppUI();

    expect(appRefreshKey.value).toBe(0);

    await vi.advanceTimersByTimeAsync(200);
    expect(appRefreshKey.value).toBe(1);
  });

  it('increments separately for calls outside the debounce window', async () => {
    panelOverlay.value = { type: 'app-ui', app: notesApp };

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI();
    await vi.advanceTimersByTimeAsync(200);
    expect(appRefreshKey.value).toBe(1);

    await refreshAppUI();
    await vi.advanceTimersByTimeAsync(200);
    expect(appRefreshKey.value).toBe(2);
  });

  it('does NOT increment when appId does not match the open app', async () => {
    panelOverlay.value = { type: 'app-ui', app: notesApp };

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI('trip-planner-2026');
    await vi.advanceTimersByTimeAsync(200);

    expect(appRefreshKey.value).toBe(0);
  });

  it('does NOT increment when no app is open and no appId given', async () => {
    const { refreshAppUI } = await import('./apps');
    await refreshAppUI();
    await vi.advanceTimersByTimeAsync(200);

    expect(appRefreshKey.value).toBe(0);
  });

  it('does NOT open the app when appId given but app not open (no surprise pop-ups)', async () => {
    expect(currentApp.value).toBeNull();

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI('notes-app');
    await vi.advanceTimersByTimeAsync(200);

    expect(currentApp.value).toBeNull();
    expect(appRefreshKey.value).toBe(0);
  });

  it('refreshes without appId when app is already open (header button)', async () => {
    panelOverlay.value = { type: 'app-ui', app: tripPlanner };

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI();
    await vi.advanceTimersByTimeAsync(200);

    expect(appRefreshKey.value).toBe(1);
    expect(currentApp.value?.id).toBe('trip-planner-2026');
  });

  it('cancels a pending debounce when a different app is opened', async () => {
    panelOverlay.value = { type: 'app-ui', app: notesApp };

    const { refreshAppUI, openApp } = await import('./apps');
    await refreshAppUI('notes-app');

    openApp(tripPlanner);
    await vi.advanceTimersByTimeAsync(200);

    expect(currentApp.value?.id).toBe('trip-planner-2026');
    expect(appRefreshKey.value).toBe(0);
  });

  describe('preserveWip option', () => {
    function seedWipThread(id: string, appId: string): void {
      const thread = makeOptimisticThreadState({
        id,
        title: 'fix it',
        channel: 'claude_code',
        initiator: 'user',
        eventsLoaded: true,
        codingAgentKind: 'app',
        codingAgentFolder: `/data/apps/${appId}`,
      });
      const next = new Map(threadMap.value);
      next.set(id, thread);
      threadMap.value = next;
    }

    beforeEach(() => {
      wipPreviewThreadId.value = null;
      threadMap.value = new Map();
      threadsLoaded.value = true;
      focusedThreadId.value = null;
    });

    afterEach(() => {
      focusedThreadId.value = null;
      threadsLoaded.value = false;
    });

    it('default refreshAppUI() drops WIP that matches the refreshed app (Apply / file-edit path)', async () => {
      seedWipThread('wip-thread-1', 'notes-app');
      panelOverlay.value = { type: 'app-ui', app: notesApp };
      focusedThreadId.value = 'wip-thread-1';
      wipPreviewThreadId.value = 'wip-thread-1';
      expect(wipPreviewThreadId.value).toBe('wip-thread-1');

      const { refreshAppUI } = await import('./apps');
      await refreshAppUI('notes-app');

      expect(wipPreviewThreadId.value).toBeNull();
    });

    it('refreshAppUI(undefined, { preserveWip: true }) keeps WIP set (header button)', async () => {
      seedWipThread('wip-thread-2', 'notes-app');
      panelOverlay.value = { type: 'app-ui', app: notesApp };
      focusedThreadId.value = 'wip-thread-2';
      wipPreviewThreadId.value = 'wip-thread-2';
      expect(wipPreviewThreadId.value).toBe('wip-thread-2');

      const { refreshAppUI } = await import('./apps');
      await refreshAppUI(undefined, { preserveWip: true });
      await vi.advanceTimersByTimeAsync(200);

      expect(wipPreviewThreadId.value).toBe('wip-thread-2');
      expect(appRefreshKey.value).toBe(1);
    });

    it('refreshAppUI(appId, { preserveWip: true }) keeps WIP set for an explicit appId', async () => {
      seedWipThread('wip-thread-3', 'notes-app');
      panelOverlay.value = { type: 'app-ui', app: notesApp };
      focusedThreadId.value = 'wip-thread-3';
      wipPreviewThreadId.value = 'wip-thread-3';
      expect(wipPreviewThreadId.value).toBe('wip-thread-3');

      const { refreshAppUI } = await import('./apps');
      await refreshAppUI('notes-app', { preserveWip: true });

      expect(wipPreviewThreadId.value).toBe('wip-thread-3');
    });
  });
});

describe('openAppById', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    inputMode.value = { type: 'do' };
    appsList.value = { status: 'not-loaded' };
    toasts.value = [];
    mockListAppsApi.mockReset();
  });

  it('toasts when apps fail to load — caller is not on the apps tab so the Loadable failed state is invisible', async () => {
    mockListAppsApi.mockRejectedValue(new Error('boom'));

    const { openAppById } = await import('./apps');
    await openAppById('notes-app');

    expect(panelOverlay.value).toBeNull();
    const errors = toasts.value.filter((t) => t.type === 'error');
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toMatch(/apps failed to load/i);
  });

  it('toasts when the app id is unknown — stale link should not silently no-op', async () => {
    appsList.value = { status: 'loaded', data: [notesApp] };
    mockListAppsApi.mockResolvedValue([notesApp]);
    const { ApiError } = await import('../../api/client/_core');
    mockGetAppApi.mockRejectedValue(new ApiError(404, 'App not found: trip-planner-2026'));

    const { openAppById } = await import('./apps');
    await openAppById('trip-planner-2026');

    expect(panelOverlay.value).toBeNull();
    const errors = toasts.value.filter((t) => t.type === 'error');
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toMatch(/trip-planner-2026/);
    expect(errors[0].message).toMatch(/no longer exists/i);
  });

  it('opens a widget by id, since the apps list never holds one (ADR 0402)', async () => {
    appsList.value = { status: 'loaded', data: [notesApp] };
    mockListAppsApi.mockResolvedValue([notesApp]);
    const widget: App = { ...notesApp, id: 'fare-grid', name: 'Fare grid', kind: 'widget', origin_thread_id: 't1' };
    mockGetAppApi.mockResolvedValue(widget);

    const { openAppById } = await import('./apps');
    await openAppById('fare-grid');

    expect(panelOverlay.value).toMatchObject({ type: 'app-ui', app: { id: 'fare-grid', kind: 'widget' } });
  });

  it('never calls an app gone when the re-scan failed, and keeps the cached list', async () => {
    appsList.value = { status: 'loaded', data: [notesApp] };
    mockListAppsApi.mockRejectedValue(new Error('boom'));

    const { openAppById } = await import('./apps');
    await openAppById('trip-planner-2026');

    expect(panelOverlay.value).toBeNull();
    expect(appsList.value).toEqual({ status: 'loaded', data: [notesApp] });
    const errors = toasts.value.filter((t) => t.type === 'error');
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toMatch(/trip-planner-2026/);
    expect(errors[0].message).not.toMatch(/no longer exists/i);
  });

  it('opens the app when found', async () => {
    appsList.value = { status: 'loaded', data: [notesApp] };

    const { openAppById } = await import('./apps');
    await openAppById('notes-app');

    expect(panelOverlay.value).toEqual({ type: 'app-ui', app: notesApp });
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });
});

const APP_DOC = 'https://host.example/ws/app/notes-app/';

describe('openApp carries an app fragment', () => {
  let origQuerySelectorAll: typeof document.querySelectorAll;
  let frameLocation: { href: string; replace: ReturnType<typeof vi.fn> };
  let framePosts: unknown[] = [];

  function mountFrame(hash: string): void {
    framePosts = [];
    frameLocation = {
      href: `${APP_DOC}${hash}`,
      replace: vi.fn((url: string) => { frameLocation.href = url; }),
    };
    const frame = {
      contentWindow: {
        location: frameLocation,
        postMessage: vi.fn((msg: unknown) => { framePosts.push(msg); }),
      },
      getBoundingClientRect: () => ({ width: 800, height: 600, top: 0, left: 0, bottom: 600, right: 800 }),
      closest: () => null,
    };
    document.querySelectorAll = vi.fn().mockReturnValue([frame]);
  }

  function deliveredFragments(): unknown[] {
    return framePosts
      .filter((m): m is { op: string; args: { fragment: unknown } } =>
        typeof m === 'object' && m !== null && (m as { op?: string }).op === 'hash')
      .map((m) => m.args.fragment);
  }

  beforeEach(() => {
    panelOverlay.value = null;
    inputMode.value = { type: 'do' };
    wipPreviewThreadId.value = null;
    appRefreshKey.value = 0;
    mockPushNavState.mockClear();
    mockReplaceNavState.mockClear();
    origQuerySelectorAll = document.querySelectorAll;
    document.querySelectorAll = vi.fn().mockReturnValue([]);
  });

  afterEach(() => {
    document.querySelectorAll = origQuerySelectorAll;
    panelOverlay.value = null;
  });

  it('stores the fragment on the overlay and feeds it to the frame src', async () => {
    const { openApp, getAppFrameSrc } = await import('./apps');
    openApp(notesApp, 'pr-1645');

    expect(panelOverlay.value).toEqual({ type: 'app-ui', app: notesApp, fragment: 'pr-1645' });
    expect(getAppFrameSrc()).toBe('/app/notes-app/#pr-1645');
  });

  it('leaves the src fragment-free when the link named no target', async () => {
    const { openApp, getAppFrameSrc } = await import('./apps');
    openApp(notesApp);

    expect(getAppFrameSrc()).toBe('/app/notes-app/');
  });

  it('puts the fragment after the WIP-preview query', async () => {
    const thread = makeOptimisticThreadState({
      id: 'wip-thread-9',
      title: 'fix it',
      channel: 'claude_code',
      initiator: 'user',
      eventsLoaded: true,
      codingAgentKind: 'app',
      codingAgentFolder: '/data/apps/notes-app',
    });
    threadMap.value = new Map([['wip-thread-9', thread]]);
    threadsLoaded.value = true;
    focusedThreadId.value = 'wip-thread-9';

    const { openApp, getAppFrameSrc } = await import('./apps');
    openApp(notesApp, 'pr-1645');
    wipPreviewThreadId.value = 'wip-thread-9';

    expect(getAppFrameSrc()).toBe('/app/notes-app/?thread_id=wip-thread-9#pr-1645');

    focusedThreadId.value = null;
    threadsLoaded.value = false;
    threadMap.value = new Map();
  });

  it('does not touch a mounted frame on a COLD open', async () => {
    mountFrame('#pr-1700');
    const { openApp } = await import('./apps');
    openApp(notesApp, 'pr-1645');

    expect(deliveredFragments()).toEqual([]);
    expect(mockReplaceNavState).not.toHaveBeenCalled();
  });

  it('hands the fragment to the live frame when the app was ALREADY open', async () => {
    const { openApp } = await import('./apps');
    openApp(notesApp, 'pr-1645');
    mountFrame('#pr-1700');

    openApp(notesApp, 'pr-1645');

    expect(deliveredFragments()).toEqual(['pr-1645']);
  });

  it('moves nobody when the second open names no target', async () => {
    const { openApp } = await import('./apps');
    openApp(notesApp, 'pr-1645');
    mountFrame('#pr-1700');

    openApp(notesApp);

    expect(deliveredFragments()).toEqual([]);
    expect(frameLocation.replace).not.toHaveBeenCalled();
  });

  it('refreshes the kept nav entry instead of adding one for a move in place', async () => {
    const { openApp } = await import('./apps');
    openApp(notesApp, 'pr-1645');
    expect(mockReplaceNavState).not.toHaveBeenCalled();

    openApp(notesApp, 'pr-1700');

    expect(mockReplaceNavState).toHaveBeenCalledTimes(1);
    expect(panelOverlay.value).toEqual({ type: 'app-ui', app: notesApp, fragment: 'pr-1700' });
  });
});

describe('openApp records one AppOpened per real open', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    inputMode.value = { type: 'do' };
    mockRecordAppOpenedApi.mockClear();
  });

  it('posts once on a fresh open', async () => {
    const { openApp } = await import('./apps');
    openApp(notesApp);

    expect(mockRecordAppOpenedApi).toHaveBeenCalledTimes(1);
    expect(mockRecordAppOpenedApi).toHaveBeenCalledWith('notes-app');
  });

  it('posts nothing for a move inside an app already open', async () => {
    const { openApp } = await import('./apps');
    openApp(notesApp);
    openApp(notesApp, 'pr-1645');
    openApp(notesApp);

    expect(mockRecordAppOpenedApi).toHaveBeenCalledTimes(1);
  });

  it('posts again when the user opens a different app', async () => {
    const { openApp } = await import('./apps');
    openApp(notesApp);
    openApp(tripPlanner);

    expect(mockRecordAppOpenedApi.mock.calls).toEqual([['notes-app'], ['trip-planner-2026']]);
  });

  it('posts nothing for a widget opened in Canvas', async () => {
    const { openApp } = await import('./apps');
    openApp({ ...notesApp, id: 'fare-grid', kind: 'widget' });

    expect(mockRecordAppOpenedApi).not.toHaveBeenCalled();
  });
});

describe('openApp expands the content pane on desktop', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    inputMode.value = { type: 'do' };
  });

  it('expands a collapsed content pane so the click is not silently absorbed', async () => {
    splitRatio.value = 1;

    const { openApp } = await import('./apps');
    openApp(notesApp);

    expect(panelOverlay.value).toEqual({ type: 'app-ui', app: notesApp });
    expect(splitRatio.value).toBeLessThan(1);
  });

  it('preserves a custom split ratio when the pane is already visible', async () => {
    splitRatio.value = 0.7;

    const { openApp } = await import('./apps');
    openApp(notesApp);

    expect(splitRatio.value).toBe(0.7);
  });
});

describe('exitAppFullscreen', () => {
  const doc = document as unknown as Record<string, unknown>;

  beforeEach(() => {
    appPseudoFullscreen.value = false;
    delete doc.fullscreenElement;
    delete doc.webkitFullscreenElement;
    delete doc.exitFullscreen;
    delete doc.webkitExitFullscreen;
  });

  afterEach(() => {
    appPseudoFullscreen.value = false;
    delete doc.fullscreenElement;
    delete doc.webkitFullscreenElement;
    delete doc.exitFullscreen;
    delete doc.webkitExitFullscreen;
  });

  it('reports nothing to leave when no app panel is fullscreen', async () => {
    const { exitAppFullscreen } = await import('./apps');
    expect(exitAppFullscreen()).toBe(false);
  });

  it('ends the CSS pseudo-fullscreen fallback', async () => {
    appPseudoFullscreen.value = true;
    const { exitAppFullscreen } = await import('./apps');
    expect(exitAppFullscreen()).toBe(true);
    expect(appPseudoFullscreen.value).toBe(false);
  });

  it('exits native fullscreen through the unprefixed API', async () => {
    doc.fullscreenElement = {};
    const exit = vi.fn().mockResolvedValue(undefined);
    doc.exitFullscreen = exit;
    const { exitAppFullscreen } = await import('./apps');
    expect(exitAppFullscreen()).toBe(true);
    expect(exit).toHaveBeenCalledTimes(1);
  });

  it('exits native fullscreen through the webkit spelling, which returns void not a promise', async () => {
    doc.webkitFullscreenElement = {};
    const exit = vi.fn();
    doc.webkitExitFullscreen = exit;
    const { exitAppFullscreen } = await import('./apps');
    expect(exitAppFullscreen()).toBe(true);
    expect(exit).toHaveBeenCalledTimes(1);
  });
});

describe('a reload restore of the open app', () => {
  it('posts nothing when a reload restores the open app', async () => {
    vi.resetModules();
    mockRecordAppOpenedApi.mockClear();
    localStorage.setItem('app-window-open', notesApp.id);
    mockListAppsApi.mockResolvedValueOnce([notesApp]);
    const store = await import('../store');
    const { loadApps } = await import('./apps');
    await loadApps();

    expect(store.panelOverlay.value).toEqual({ type: 'app-ui', app: notesApp });
    expect(mockRecordAppOpenedApi).not.toHaveBeenCalled();
    localStorage.removeItem('app-window-open');
  });
});
