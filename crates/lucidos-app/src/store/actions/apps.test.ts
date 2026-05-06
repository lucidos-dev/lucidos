import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { panelOverlay, currentApp, appsList, inputMode, appRefreshKey, splitRatio } from '../store';
import type { App } from '../types';

// Mock API client
const mockPostAppCapture = vi.fn().mockResolvedValue(undefined);
const mockListAppsApi = vi.fn().mockResolvedValue([]);
vi.mock('../../api/client', () => ({
  postAppCapture: (...args: unknown[]) => mockPostAppCapture(...args),
  listAppsApi: (...args: unknown[]) => mockListAppsApi(...args),
  appUrl: vi.fn((id: string) => `/api/app/${id}/`),
}));

vi.mock('./navigation', () => ({
  pushNavState: vi.fn(),
}));

const notesApp: App = {
  id: 'notes-app',
  name: 'Notes App',
  description: 'Daily notes',
  knowhow: [],
};

const tripPlanner: App = {
  id: 'trip-planner-2026',
  name: 'Trip Planner 2026',
  description: 'Vacation planner',
  knowhow: [],
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
    // Three RefreshAppUI events firing in quick succession (e.g. the agentic
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
    // the post-loop RefreshAppUI surprised the user by opening the FINN app.
    // RefreshAppUI must mean "reload if currently open", never "open from
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
    // Pending RefreshAppUI for app A must not fire after the user switches to
    // app B — otherwise B's iframe gets a stray refresh keyed to A's edit.
    panelOverlay.value = { type: 'app-ui', app: notesApp };

    const { refreshAppUI, openApp } = await import('./apps');
    await refreshAppUI('notes-app');

    openApp(tripPlanner);
    await vi.advanceTimersByTimeAsync(200);

    expect(currentApp.value?.id).toBe('trip-planner-2026');
    expect(appRefreshKey.value).toBe(0);
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
