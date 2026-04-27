import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { panelOverlay, currentApp, appsList, inputMode, appRefreshKey } from '../store';
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

const morgenlogg: App = {
  id: 'morgenlogg',
  name: 'Morgenlogg',
  description: 'Morning log',
  knowhow: [],
};

const sommerferie: App = {
  id: 'sommerferie-2026',
  name: 'Sommerferie 2026',
  description: 'Summer vacation planner',
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
      data: [morgenlogg, sommerferie],
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

  it('does NOT auto-open a different app when no iframe exists (the Morgenlogg bug)', async () => {
    // panelOverlay starts null (set in beforeEach) — no app-ui iframe in DOM.
    // LLM calls capture_app(app_id="morgenlogg") — no iframe in DOM.
    // BUG: captureAppUI used to call openApp(morgenlogg), replacing the user's panel state.
    const { captureAppUI } = await import('./apps');
    await captureAppUI('morgenlogg', 'test-request-id');

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
    await captureAppUI('morgenlogg', 'test-request-id');

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
      data: [morgenlogg, sommerferie],
    };
    mockListAppsApi.mockResolvedValue([morgenlogg, sommerferie]);
    // Stub DOM queries — not needed for signal-based tests
    origQuerySelectorAll = document.querySelectorAll;
    document.querySelectorAll = vi.fn().mockReturnValue([]);
  });

  afterEach(() => {
    vi.useRealTimers();
    document.querySelectorAll = origQuerySelectorAll;
  });

  it('increments appRefreshKey when the target app is open', async () => {
    panelOverlay.value = { type: 'app-ui', app: morgenlogg };
    expect(appRefreshKey.value).toBe(0);

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI('morgenlogg');
    await vi.advanceTimersByTimeAsync(200);

    expect(appRefreshKey.value).toBe(1);
  });

  it('debounces multiple rapid calls into a single reload', async () => {
    // Three RefreshAppUI events firing in quick succession (e.g. the agentic
    // loop emits one per modified app + an explicit refresh_app) must collapse
    // into ONE iframe reload — otherwise the iframe is bombarded mid-navigation.
    panelOverlay.value = { type: 'app-ui', app: morgenlogg };

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
    panelOverlay.value = { type: 'app-ui', app: morgenlogg };

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI();
    await vi.advanceTimersByTimeAsync(200);
    expect(appRefreshKey.value).toBe(1);

    await refreshAppUI();
    await vi.advanceTimersByTimeAsync(200);
    expect(appRefreshKey.value).toBe(2);
  });

  it('does NOT increment when appId does not match the open app', async () => {
    panelOverlay.value = { type: 'app-ui', app: morgenlogg };

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI('sommerferie-2026');
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

  it('opens the app and refreshes when appId given but app not open', async () => {
    // No app currently open
    expect(currentApp.value).toBeNull();

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI('morgenlogg');
    await vi.advanceTimersByTimeAsync(200);

    // Should have opened the app
    expect(currentApp.value?.id).toBe('morgenlogg');
    // Should have refreshed
    expect(appRefreshKey.value).toBe(1);
  });

  it('refreshes without appId when app is already open (header button)', async () => {
    panelOverlay.value = { type: 'app-ui', app: sommerferie };

    const { refreshAppUI } = await import('./apps');
    await refreshAppUI();
    await vi.advanceTimersByTimeAsync(200);

    expect(appRefreshKey.value).toBe(1);
    // Should not have changed the open app
    expect(currentApp.value?.id).toBe('sommerferie-2026');
  });

  it('cancels a pending debounce when a different app is opened', async () => {
    // Pending RefreshAppUI for app A must not fire after the user switches to
    // app B — otherwise B's iframe gets a stray refresh keyed to A's edit.
    panelOverlay.value = { type: 'app-ui', app: morgenlogg };

    const { refreshAppUI, openApp } = await import('./apps');
    await refreshAppUI('morgenlogg');

    openApp(sommerferie);
    await vi.advanceTimersByTimeAsync(200);

    expect(currentApp.value?.id).toBe('sommerferie-2026');
    expect(appRefreshKey.value).toBe(0);
  });
});
