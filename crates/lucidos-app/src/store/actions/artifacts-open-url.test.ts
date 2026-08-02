import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { panelOverlay, preferences, webviewInitialUrl } from '../store';

// Spy on the panel side effects — real implementations dirty localStorage,
// a module-level nav stack, and touch viewport/DOM state. artifacts.ts imports
// only pushNavState from './navigation' (it defines its own normalizeUrl).
const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));
const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane }));

const platformMocks = vi.hoisted(() => ({ isTauri: false, isIOSPwa: false }));
vi.mock('../../utils/platform', () => ({
  isTauri: () => platformMocks.isTauri,
  isIOS: () => false,
  // Read by openExternalUrl (the non-Tauri branch of openUrl).
  isIOSPwa: () => platformMocks.isIOSPwa,
}));

// openExternal is the OS opener (system browser). setTitlebarColor is unused
// here but the real preferences module (loaded via artifacts → currentInAppBrowser)
// imports it from this module, so the mock must provide it.
const openExternal = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('../../utils/tauri', () => ({
  openExternal,
  setTitlebarColor: () => Promise.resolve(),
}));

// Imports must come after vi.mock so the mocked deps are wired in.
const { openUrl } = await import('./artifacts');

// jsdom doesn't implement window.open, so stub it as a global rather than
// spying on a non-existent property.
const windowOpen = vi.hoisted(() => vi.fn());

const TARGET_URL = 'https://example.com/';
const APP_URL = 'https://app.example.com/ws/dev/';
let fakeLocation: { href: string };

describe('openUrl — system browser vs in-app webview routing', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    webviewInitialUrl.value = null;
    preferences.value = { status: 'loaded', data: {} };
    platformMocks.isTauri = false;
    platformMocks.isIOSPwa = false;
    pushNavState.mockClear();
    revealContentPane.mockClear();
    openExternal.mockClear();
    windowOpen.mockClear();
    fakeLocation = { href: APP_URL };
    vi.stubGlobal('open', windowOpen);
    vi.stubGlobal('location', fakeLocation);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('non-Tauri (web/PWA): opens a new tab, never the in-app panel or the OS opener', () => {
    platformMocks.isTauri = false;
    openUrl(TARGET_URL);

    expect(window.open).toHaveBeenCalledWith(TARGET_URL, '_blank', 'noopener');
    expect(openExternal).not.toHaveBeenCalled();
    expect(panelOverlay.value).toBeNull();
    expect(window.location.href).toBe(APP_URL);
  });

  it('installed iOS PWA: hands the URL to Safari, never the inescapable in-app web view', () => {
    platformMocks.isTauri = false;
    platformMocks.isIOSPwa = true;

    openUrl(TARGET_URL);

    expect(window.location.href).toBe(`x-safari-${TARGET_URL}`);
    expect(window.open).not.toHaveBeenCalled();
    expect(openExternal).not.toHaveBeenCalled();
    expect(panelOverlay.value).toBeNull();
  });

  it('Tauri + toggle off (default): opens the system browser, never the in-app panel', () => {
    platformMocks.isTauri = true;
    preferences.value = { status: 'loaded', data: {} }; // experimental_in_app_browser unset → off

    openUrl(TARGET_URL);

    expect(openExternal).toHaveBeenCalledWith(TARGET_URL);
    expect(panelOverlay.value).toBeNull();
    expect(window.open).not.toHaveBeenCalled();
    expect(pushNavState).not.toHaveBeenCalled();
    expect(window.location.href).toBe(APP_URL);
  });

  it('Tauri + toggle on: opens the in-app webview panel, never the system browser', () => {
    platformMocks.isTauri = true;
    preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };

    openUrl(TARGET_URL);

    expect(panelOverlay.value).toEqual({ type: 'url-preview', url: TARGET_URL });
    expect(webviewInitialUrl.value).toBe(TARGET_URL);
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(openExternal).not.toHaveBeenCalled();
    expect(window.location.href).toBe(APP_URL);
  });
});
