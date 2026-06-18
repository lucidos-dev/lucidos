import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { preferences } from '../store';
import { applyTheme, applyFontFamily, applyUiScale, currentTheme, loadPreferences, welcomeSuggestionsDismissed, dismissWelcomeSuggestions } from './preferences';
import * as apiClient from '../../api/client';

const platformMocks = vi.hoisted(() => ({ isIOS: false }));
vi.mock('../../utils/platform', () => ({
  isIOS: () => platformMocks.isIOS,
}));

describe('currentTheme — localStorage fallback', () => {
  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  it('returns localStorage theme when backend has no theme preference', () => {
    // User set light mode → saved in localStorage + backend
    // Backend lost the preference (device_id change, save failure, etc.)
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'loaded', data: { 'font-family': 'monospace' } };

    // currentTheme() must respect localStorage, not default to 'dark'
    expect(currentTheme()).toBe('light');
  });

  it('returns backend theme when backend has theme preference', () => {
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'loaded', data: { theme: 'dark' } };

    // Backend is source of truth when it has a value
    expect(currentTheme()).toBe('dark');
  });

  it('returns localStorage theme when preferences not yet loaded', () => {
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'loading' };

    expect(currentTheme()).toBe('light');
  });

  it('returns dark as final fallback when nothing is set', () => {
    preferences.value = { status: 'loaded', data: {} };

    expect(currentTheme()).toBe('dark');
  });

  it('returns system from localStorage when backend has no theme', () => {
    localStorage.setItem('lucidos-theme', 'system');
    preferences.value = { status: 'loaded', data: {} };

    expect(currentTheme()).toBe('system');
  });

  it('returns localStorage theme when preferences failed to load', () => {
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'failed', error: 'network error' };

    expect(currentTheme()).toBe('light');
  });

  it('skips invalid backend value and falls back to localStorage', () => {
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'loaded', data: { theme: 'garbage' } };

    expect(currentTheme()).toBe('light');
  });

  it('ignores invalid localStorage values', () => {
    localStorage.setItem('lucidos-theme', 'purple');
    preferences.value = { status: 'loaded', data: {} };

    expect(currentTheme()).toBe('dark');
  });
});

describe('apply* functions mirror to localStorage for FOUC inline script', () => {
  // The inline FOUC IIFE in index.html reads these localStorage keys on next
  // page load. Each apply* mutator must keep its key fresh so the next reload
  // paints the right value before any stylesheet evaluates.

  let inlineProps: Record<string, string>;
  let attrs: Record<string, string>;
  let originalStyle: any;
  let originalSetAttribute: any;
  let originalGetAttribute: any;
  let originalRemoveAttribute: any;

  beforeEach(() => {
    localStorage.clear();
    inlineProps = {};
    attrs = {};
    const el = (document as any).documentElement;
    originalStyle = el.style;
    originalSetAttribute = el.setAttribute;
    originalGetAttribute = el.getAttribute;
    originalRemoveAttribute = el.removeAttribute;
    el.style = {
      setProperty: (k: string, v: string) => { inlineProps[k] = v; },
      getPropertyValue: (k: string) => inlineProps[k] ?? '',
      removeProperty: (k: string) => { delete inlineProps[k]; },
      colorScheme: '',
      background: '',
    };
    el.setAttribute = (k: string, v: string) => { attrs[k] = v; };
    el.getAttribute = (k: string) => attrs[k] ?? null;
    el.removeAttribute = (k: string) => { delete attrs[k]; };
  });

  afterEach(() => {
    const el = (document as any).documentElement;
    el.style = originalStyle;
    el.setAttribute = originalSetAttribute;
    el.getAttribute = originalGetAttribute;
    el.removeAttribute = originalRemoveAttribute;
  });

  it('applyTheme writes lucidos-theme to localStorage', () => {
    applyTheme('light');
    expect(localStorage.getItem('lucidos-theme')).toBe('light');
    expect(attrs['data-theme']).toBe('light');
    expect(inlineProps['--bg-primary']).toBe('#ffffff');
  });

  it('applyTheme keeps inline --bg-primary in sync on toggle', () => {
    applyTheme('light');
    expect(inlineProps['--bg-primary']).toBe('#ffffff');
    applyTheme('dark');
    expect(inlineProps['--bg-primary']).toBe('#0b2342');
  });

  it('applyTheme sets html.style.background inline so the WebView has a paintable bg before global.css loads', () => {
    // See preferences.ts:applyTheme for why — iOS PWA cold-restart flash.
    const el = (document as any).documentElement;
    applyTheme('dark');
    expect(el.style.background).toBe('#0b2342');
    applyTheme('light');
    expect(el.style.background).toBe('#ffffff');
  });

  it('applyFontFamily writes lucidos-font-family to localStorage', () => {
    // Use 'monospace' (no Google Font load) — `inter` would trigger
    // ensureFontLoaded, which calls document.head.appendChild and is
    // orthogonal to what this test verifies.
    applyFontFamily('monospace');
    expect(localStorage.getItem('lucidos-font-family')).toBe('monospace');
    expect(inlineProps['--font-ui']).toContain('SF Mono');
  });

  it('applyUiScale writes lucidos-ui-scale to localStorage', () => {
    applyUiScale(125);
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('125');
    expect(inlineProps['--user-ui-scale']).toBe('125%');
  });

  it('applyUiScale clamps and stores the clamped value', () => {
    applyUiScale(500);
    // UI_SCALE_MAX = 200
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('200');
  });

  it('applyUiScale snaps off-grid values (115 → 112.5)', () => {
    applyUiScale(115);
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('112.5');
    expect(inlineProps['--user-ui-scale']).toBe('112.5%');
  });
});

describe('loadPreferences — no flash when refetching after PreferencesChanged', () => {
  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('does not wipe loaded state to "loading" on a refetch', async () => {
    // First load → goes through 'loading' to 'loaded' as expected.
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({
      preferences: { theme: 'dark', 'font-family': 'monospace' },
    });
    await loadPreferences();
    expect(preferences.value.status).toBe('loaded');

    // Refetch — should NOT flip back to 'loading'. We assert the value never
    // becomes 'loading' between the call site and the resolved value swap.
    let observedLoadingDuringRefetch = false;
    const reloadPromise = (async () => {
      const promise = loadPreferences();
      // Inspect state synchronously after the function starts but before it
      // resolves. Since `loadPreferences` only flips when status is
      // 'not-loaded', the synchronous check must still see 'loaded'.
      if ((preferences.value as { status: string }).status === 'loading') {
        observedLoadingDuringRefetch = true;
      }
      await promise;
    })();
    await reloadPromise;

    expect(observedLoadingDuringRefetch).toBe(false);
    expect(preferences.value.status).toBe('loaded');
  });

  it('still flips to "loading" on the very first call', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockImplementation(async () => {
      // Capture the synchronous state after invocation.
      return { preferences: { theme: 'dark' } };
    });

    const promise = loadPreferences();
    expect(preferences.value.status).toBe('loading');
    await promise;
    expect(preferences.value.status).toBe('loaded');
  });
});

describe('loadPreferences — skip re-apply when theme unchanged (iOS matchMedia flash)', () => {
  // Background: iOS WKWebView's matchMedia for prefers-color-scheme returns
  // wrong synchronous values at random points post-FOUC. Re-applying 'system'
  // on every PreferencesChanged briefly flipped the page; this guard skips
  // the re-apply when the stored preference matches what was last applied.
  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('does not rewrite data-theme on a refetch when the theme value matches the previously applied value', async () => {
    // Pin the "previously applied" value via a direct applyTheme so this test
    // is independent of any state left by earlier tests in the file.
    applyTheme('light');

    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({
      preferences: { theme: 'light' },
    });

    const setAttrSpy = vi.spyOn(document.documentElement, 'setAttribute');
    await loadPreferences();

    const themeWrites = setAttrSpy.mock.calls.filter((c) => c[0] === 'data-theme');
    expect(themeWrites).toHaveLength(0);
  });

  it('still rewrites data-theme when the value differs from the previously applied value', async () => {
    applyTheme('light');

    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({
      preferences: { theme: 'dark' },
    });

    const setAttrSpy = vi.spyOn(document.documentElement, 'setAttribute');
    await loadPreferences();

    const themeWrites = setAttrSpy.mock.calls.filter((c) => c[0] === 'data-theme');
    expect(themeWrites.length).toBeGreaterThan(0);
  });
});

describe("applyTheme('system') — matchMedia change listener gating", () => {
  let originalMatchMedia: typeof window.matchMedia;
  let listeners: Array<(e: { matches: boolean }) => void>;
  let mqLight: boolean;

  beforeEach(() => {
    platformMocks.isIOS = false;
    listeners = [];
    mqLight = false;
    originalMatchMedia = window.matchMedia;
    (window as any).matchMedia = () => ({
      get matches() { return mqLight; },
      addEventListener: (_t: string, fn: (e: { matches: boolean }) => void) => {
        listeners.push(fn);
      },
      removeEventListener: (_t: string, fn: (e: { matches: boolean }) => void) => {
        const i = listeners.indexOf(fn);
        if (i >= 0) listeners.splice(i, 1);
      },
    });
  });

  afterEach(() => {
    (window as any).matchMedia = originalMatchMedia;
    platformMocks.isIOS = false;
  });

  it('off iOS: subscribes to prefers-color-scheme change events and re-applies theme', () => {
    applyTheme('system');
    expect(listeners).toHaveLength(1);

    const setAttrSpy = vi.spyOn(document.documentElement, 'setAttribute');
    mqLight = true;
    for (const fn of listeners) fn({ matches: true });

    const themeWrites = setAttrSpy.mock.calls.filter((c) => c[0] === 'data-theme');
    expect(themeWrites.length).toBeGreaterThan(0);
  });

  it('on iOS: skips the listener entirely (WKWebView fires wrong values)', () => {
    platformMocks.isIOS = true;
    applyTheme('system');
    expect(listeners).toHaveLength(0);
  });
});

describe('welcomeSuggestionsDismissed — new-workspace welcome gate', () => {
  beforeEach(() => {
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('fails closed while preferences are not loaded (no flash for returning users)', () => {
    expect(welcomeSuggestionsDismissed()).toBe(true);
  });

  it('fails closed when preferences failed to load', () => {
    preferences.value = { status: 'failed', error: 'network error' };
    expect(welcomeSuggestionsDismissed()).toBe(true);
  });

  it('is NOT dismissed on a loaded workspace with the preference unset (new workspace shows welcome)', () => {
    preferences.value = { status: 'loaded', data: {} };
    expect(welcomeSuggestionsDismissed()).toBe(false);
  });

  it('is dismissed once the preference is set to true', () => {
    preferences.value = { status: 'loaded', data: { welcome_suggestions_dismissed: 'true' } };
    expect(welcomeSuggestionsDismissed()).toBe(true);
  });

  it('dismissWelcomeSuggestions writes the preference when not yet dismissed', async () => {
    preferences.value = { status: 'loaded', data: {} };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);

    await dismissWelcomeSuggestions();

    expect(spy).toHaveBeenCalledWith('welcome_suggestions_dismissed', 'true', undefined);
    expect((preferences.value as { data: Record<string, string> }).data.welcome_suggestions_dismissed).toBe('true');
  });

  it('dismissWelcomeSuggestions is idempotent — skips the write when already dismissed', async () => {
    preferences.value = { status: 'loaded', data: { welcome_suggestions_dismissed: 'true' } };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);

    await dismissWelcomeSuggestions();

    expect(spy).not.toHaveBeenCalled();
  });
});
