import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { panelOverlay, currentApp, appsList, appPseudoFullscreen, inputMode, appRefreshKey, splitRatio, toasts, wipPreviewThreadId, threadMap, focusedThreadId, threadsLoaded } from '../store';
import type { App } from '../types';
import { makeOptimisticThreadState } from '../thread-events';
// Importing the wipPreview module installs the auto-revert effect that
// clears WIP when focusedThreadId / threadMap / currentApp drift out of
// sync. The preserveWip tests need that effect installed so they exercise
// the same coordination the live app does.
import '../actions/wipPreview';

// Mock API client
const mockPostAppCapture = vi.fn().mockResolvedValue(undefined);
const mockListAppsApi = vi.fn().mockResolvedValue([]);
vi.mock('../../api/client', () => ({
  postAppCapture: (...args: unknown[]) => mockPostAppCapture(...args),
  listAppsApi: (...args: unknown[]) => mockListAppsApi(...args),
  appUrl: vi.fn((id: string) => `/app/${id}/`),
}));

vi.mock('./navigation', () => ({
  pushNavState: vi.fn(),
}));

const notesApp: App = {
  id: 'notes-app',
  name: 'Notes App',
  description: 'Daily notes',
};

const tripPlanner: App = {
  id: 'trip-planner-2026',
  name: 'Trip Planner 2026',
  description: 'Vacation planner',
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

    // Stub DOM queries to simulate "no iframe in DOM"
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
    // panelOverlay starts null (set in beforeEach) — no app-ui iframe in DOM.
    // LLM calls capture_app(app_id="notes-app") — no iframe in DOM.
    // BUG: captureAppUI used to call openApp(notesApp), replacing the user's panel state.
    const { captureAppUI } = await import('./apps');
    await captureAppUI('notes-app', 'test-request-id');

    // FIX: panelOverlay must NOT be changed by capture.
    expect(panelOverlay.value).toBeNull();
    expect(currentApp.value).toBeNull();

    // Verify the API was called with an error (no iframe to capture)
    expect(mockPostAppCapture).toHaveBeenCalledWith(
      'test-request-id',
      '',
      expect.stringContaining('Error'),
    );
  });

  it('does NOT auto-open an app when user has a different panel open', async () => {
    // User is viewing file preview — no app-ui iframe in DOM
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };

    const { captureAppUI } = await import('./apps');
    await captureAppUI('notes-app', 'test-request-id');

    // Panel should NOT be changed to app-ui
    expect(panelOverlay.value).toEqual({ type: 'file-preview', path: 'artifacts/notes.md' });

    expect(mockPostAppCapture).toHaveBeenCalledWith(
      'test-request-id',
      '',
      expect.stringContaining('Error'),
    );
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
    // Stub DOM queries — not needed for signal-based tests
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
    // Three AppUiRefreshRequested events firing in quick succession (e.g. the agentic
    // loop emits one per modified app + an explicit refresh_app) must collapse
    // into ONE iframe reload — otherwise the iframe is bombarded mid-navigation.
    panelOverlay.value = { type: 'app-ui', app: notesApp };

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI();
    await refreshAppUI();
    await refreshAppUI();

    // Before debounce timer fires, no increment yet
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

    // Wrong app — should not refresh
    expect(appRefreshKey.value).toBe(0);
  });

  it('does NOT increment when no app is open and no appId given', async () => {
    // No app open, no ID — nothing to refresh
    const { refreshAppUI } = await import('./apps');
    await refreshAppUI();
    await vi.advanceTimersByTimeAsync(200);

    expect(appRefreshKey.value).toBe(0);
  });

  it('does NOT open the app when appId given but app not open (no surprise pop-ups)', async () => {
    // BUG: when the LLM edited a file under apps/finn-jobs/ in a chat thread,
    // the post-loop AppUiRefreshRequested surprised the user by opening the FINN app.
    // AppUiRefreshRequested must mean "reload if currently open", never "open from
    // closed". User-initiated opens go through openApp / app-link clicks /
    // navigate_ui — refresh is for already-open iframes only.
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
    // Should not have changed the open app
    expect(currentApp.value?.id).toBe('trip-planner-2026');
  });

  it('cancels a pending debounce when a different app is opened', async () => {
    // Pending AppUiRefreshRequested for app A must not fire after the user switches to
    // app B — otherwise B's iframe gets a stray refresh keyed to A's edit.
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
      // The wipPreview effect clears WIP whenever focusedThreadId drifts from
      // the WIP thread id. Each test pins focused to its WIP thread so the
      // effect's auto-revert doesn't shadow whatever refreshAppUI does.
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
      // And still bumps the refresh key so the iframe reloads.
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

    const { openAppById } = await import('./apps');
    await openAppById('trip-planner-2026');

    expect(panelOverlay.value).toBeNull();
    const errors = toasts.value.filter((t) => t.type === 'error');
    expect(errors).toHaveLength(1);
    // After the disk re-scan still misses, the toast names the id + that it's gone.
    expect(errors[0].message).toMatch(/trip-planner-2026/);
    expect(errors[0].message).toMatch(/no longer exists/i);
  });

  it('opens the app when found', async () => {
    appsList.value = { status: 'loaded', data: [notesApp] };

    const { openAppById } = await import('./apps');
    await openAppById('notes-app');

    expect(panelOverlay.value).toEqual({ type: 'app-ui', app: notesApp });
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });
});

describe('openApp expands the content pane on desktop', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    inputMode.value = { type: 'do' };
  });

  it('expands a collapsed content pane so the click is not silently absorbed', async () => {
    // Desktop: chat full-width, content pane collapsed.
    splitRatio.value = 1;

    const { openApp } = await import('./apps');
    openApp(notesApp);

    expect(panelOverlay.value).toEqual({ type: 'app-ui', app: notesApp });
    expect(splitRatio.value).toBeLessThan(1);
  });

  it('preserves a custom split ratio when the pane is already visible', async () => {
    // User has dragged the divider to a non-default ratio — don't snap it.
    splitRatio.value = 0.7;

    const { openApp } = await import('./apps');
    openApp(notesApp);

    expect(splitRatio.value).toBe(0.7);
  });
});

describe('exitAppFullscreen', () => {
  // The `document` stub in test-setup.ts is a plain object, so the fullscreen
  // surface is whatever a test puts on it. Restore per test.
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
    // A bare `.then` on the prefixed call throws a TypeError synchronously, so
    // the void return is the case worth pinning.
    doc.webkitFullscreenElement = {};
    const exit = vi.fn();
    doc.webkitExitFullscreen = exit;
    const { exitAppFullscreen } = await import('./apps');
    expect(exitAppFullscreen()).toBe(true);
    expect(exit).toHaveBeenCalledTimes(1);
  });
});
