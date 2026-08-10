import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { preferences, toasts } from '../store';
import { applyTheme, applyFontFamily, applyUiScale, currentTheme, loadPreferences, welcomeSuggestionsDismissed, dismissWelcomeSuggestions, currentInAppBrowser, setInAppBrowser, inAppBrowserAvailable, currentExternalLinkTarget, setExternalLinkTarget, externalLinkTargetConfigurable, savePreference, flushPendingPreferenceWrites, _pendingPreferenceKeysForTesting, _resetPendingPreferenceWritesForTesting, currentMaxToolCalls, estimateTurnDuration, MAX_TOOL_CALLS_DEFAULT, MAX_TOOL_CALLS_MIN, isBackupScheduleActive, backupIsActive, backupReminderHiddenByDismissal, backupReminderNextDismissal, backupReminderVisibleIn, backupReminderVisible, dismissBackupReminder, BACKUP_REMINDER_FOREVER, BACKUP_REMINDER_SNOOZE_MS } from './preferences';
import * as apiClient from '../../api/client';
import { ApiError } from '../../api/client';
import type { ApiResult } from '../../api/types';

const platformMocks = vi.hoisted(() => ({ isIOS: false, isTauri: false, isIOSPwa: false }));
vi.mock('../../utils/platform', () => ({
  isIOS: () => platformMocks.isIOS,
  isTauri: () => platformMocks.isTauri,
  isIOSPwa: () => platformMocks.isIOSPwa,
}));

// applyTheme tints the native title bar via this when isTauri(); mock it so the
// web-path tests don't need a Tauri IPC bridge and the Tauri-path test can
// assert the per-theme color.
const setTitlebarColorMock = vi.hoisted(() => vi.fn(() => Promise.resolve()));
// The same block signals that the page is about to paint, which is what lets the
// shell show the window it kept hidden at launch. The real one is one-shot per
// document (utils/tauri.test.ts pins that); this mock counts every call, which is
// how the tests below can see it fire on the Tauri path only.
const windowReadyToShowMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/tauri', () => ({
  setTitlebarColor: setTitlebarColorMock,
  windowReadyToShow: windowReadyToShowMock,
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
  let originalHead: any;

  beforeEach(() => {
    localStorage.clear();
    inlineProps = {};
    attrs = {};
    // A Google-font preference calls ensureFontLoaded, which appends a <link>.
    // The test-setup document stub has no `head`, so give it one: the font
    // fetch is orthogonal to what this block asserts, but the fira-code tests
    // below cannot avoid triggering it.
    originalHead = (document as any).head;
    (document as any).head = { appendChild: () => {} };
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
    (document as any).head = originalHead;
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
    expect(inlineProps['--bg-primary']).toBe('#07172e');
  });

  it('applyTheme sets html.style.background inline so the WebView has a paintable bg before global.css loads', () => {
    // See preferences.ts:applyTheme for why — iOS PWA cold-restart flash.
    const el = (document as any).documentElement;
    applyTheme('dark');
    expect(el.style.background).toBe('#07172e');
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

  it('applyFontFamily turns Fira Code ligatures OFF for text with explicit zeros, not `normal`', () => {
    // `normal` does NOT disable ligatures. `liga` and `calt` are default-ON
    // features in CSS, so `normal` means "the font's defaults" and renders
    // BYTE-IDENTICALLY to `"liga" 1, "calt" 1` (established by pixel
    // comparison in headless Chromium, since the computed value shows no
    // difference). An earlier attempt at this fix merely stopped setting the
    // property, which left the defaults in place and changed nothing at all.
    applyFontFamily('fira-code');
    expect(inlineProps['--font-features-text']).toBe('"liga" 0, "calt" 0');
    // Scope is decided by CSS, so no set-point writes the bare property.
    expect(inlineProps['font-feature-settings']).toBeUndefined();
  });

  it('applyFontFamily turns them back ON for code', () => {
    // Code surfaces inherit the OFF value now, so they must re-enable
    // explicitly. `normal` would work today (defaults are on) but re-encodes
    // the exact trap above, so the value is spelled out.
    applyFontFamily('fira-code');
    expect(inlineProps['--font-features-code']).toBe('"liga" 1, "calt" 1');
  });

  it('applyFontFamily resolves BOTH properties to normal for every other font', () => {
    // Switching away from Fira Code must clear both rather than leave a stale
    // value on <html>. `normal` is right HERE: a non-Fira font wants its own
    // defaults untouched, and an unconditional `"liga" 0` would also kill the
    // fi/fl ligatures a proportional font like Inter legitimately wants.
    applyFontFamily('fira-code');
    applyFontFamily('monospace');
    expect(inlineProps['--font-features-text']).toBe('normal');
    expect(inlineProps['--font-features-code']).toBe('normal');
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

describe('applyTheme — native title-bar tint (Tauri)', () => {
  beforeEach(() => {
    setTitlebarColorMock.mockClear();
    windowReadyToShowMock.mockClear();
    platformMocks.isTauri = false;
  });

  afterEach(() => {
    platformMocks.isTauri = false;
  });

  it('does not tint the title bar outside Tauri (web / PWA)', () => {
    applyTheme('light');
    expect(setTitlebarColorMock).not.toHaveBeenCalled();
  });

  it('tints the title bar the header-top blue per theme inside Tauri', () => {
    platformMocks.isTauri = true;
    // Mirrors --header-gradient's top stop in styles/global/base.css.
    applyTheme('light');
    expect(setTitlebarColorMock).toHaveBeenLastCalledWith('#1a6fd0');
    applyTheme('dark');
    expect(setTitlebarColorMock).toHaveBeenLastCalledWith('#15549e');
  });

  it('tells the shell the page is ready to show, Tauri only', () => {
    // Nothing is waiting on a hidden window in a browser, and there is no IPC
    // bridge to carry the signal.
    applyTheme('light');
    expect(windowReadyToShowMock).not.toHaveBeenCalled();

    // Under Tauri the theme is now resolved and on the document, which is the
    // moment a window can come on screen showing a page instead of bare tint.
    platformMocks.isTauri = true;
    applyTheme('light');
    expect(windowReadyToShowMock).toHaveBeenCalled();
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

/**
 * The app-shell backup reminder's visibility rule.
 *
 * The property worth pinning is that it needs no endpoint of its own: the
 * engine's GET /backup/schedule reports a `schedule` only when the cron is
 * active AND a provider is set (`api::backup::schedule_response`), and both are
 * ordinary preference rows that GET /preferences already returns. So these
 * predicates are a mirror of an engine rule, and drift between them is the bug
 * to catch. The component-side tests live in
 * `components/layout/__tests__/backup-reminder-banner.test.tsx`.
 *
 * Only the `schedule` field is mirrored. That response's `provider` field is
 * reported whether or not the schedule is active, because a destination does
 * not stop existing when the cron is off, so it is NOT a signal for "is backup
 * on" and this predicate deliberately does not follow it.
 */
describe('backup reminder: is backup actually on?', () => {
  const T0 = Date.parse('2026-08-04T09:00:00Z');
  const DAY_MS = 24 * 60 * 60 * 1000;
  /** A workspace with automatic backup switched on. */
  const BACKUP_ON = { backup_schedule: '0 0 3 * * *', backup_provider: 'google_drive' };

  beforeEach(() => {
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('mirrors core::backup::is_schedule_active for the cron half', () => {
    expect(isBackupScheduleActive(undefined)).toBe(false);
    expect(isBackupScheduleActive('')).toBe(false);
    expect(isBackupScheduleActive('off')).toBe(false);
    expect(isBackupScheduleActive('0 0 3 * * *')).toBe(true);
    expect(isBackupScheduleActive('0 0 */12 * * *')).toBe(true);
  });

  it('requires BOTH an active cron and a provider', () => {
    expect(backupIsActive(BACKUP_ON)).toBe(true);
    expect(backupIsActive({})).toBe(false);
    // A cron with nowhere to upload to.
    expect(backupIsActive({ backup_schedule: '0 0 3 * * *' })).toBe(false);
    // The shape left behind by picking a provider in Settings and never
    // choosing a schedule, which backs up nothing.
    expect(backupIsActive({ backup_provider: 'google_drive' })).toBe(false);
    expect(backupIsActive({ backup_provider: 'google_drive', backup_schedule: 'off' })).toBe(false);
  });

  it('an unset dismissal is not a dismissal', () => {
    expect(backupReminderHiddenByDismissal(undefined, T0)).toBe(false);
    expect(backupReminderHiddenByDismissal('', T0)).toBe(false);
  });

  it('the first dismissal hides it for 30 days and no longer', () => {
    const at = new Date(T0).toISOString();
    expect(backupReminderHiddenByDismissal(at, T0)).toBe(true);
    expect(backupReminderHiddenByDismissal(at, T0 + 29 * DAY_MS)).toBe(true);
    expect(backupReminderHiddenByDismissal(at, T0 + BACKUP_REMINDER_SNOOZE_MS)).toBe(false);
    expect(backupReminderHiddenByDismissal(at, T0 + 31 * DAY_MS)).toBe(false);
  });

  it('"forever" hides regardless of how long ago it was set', () => {
    expect(backupReminderHiddenByDismissal(BACKUP_REMINDER_FOREVER, T0)).toBe(true);
    expect(backupReminderHiddenByDismissal(BACKUP_REMINDER_FOREVER, T0 + 400 * DAY_MS)).toBe(true);
  });

  it('an unparseable dismissal fails towards SHOWING the warning', () => {
    // Garbage can only arrive by hand-writing the preference. This is a
    // data-loss warning, so an uninterpretable dismissal must not silence it,
    // and the next dismiss overwrites the garbage with a real instant.
    expect(backupReminderHiddenByDismissal('yesterday', T0)).toBe(false);
    expect(backupReminderNextDismissal('yesterday', T0)).toBe(new Date(T0).toISOString());
  });

  it('records the instant on the first dismissal and forever on the second', () => {
    expect(backupReminderNextDismissal(undefined, T0)).toBe(new Date(T0).toISOString());
    expect(backupReminderNextDismissal('', T0)).toBe(new Date(T0).toISOString());
    // The banner can only be back on screen because the snooze lapsed, so
    // dismissing it again is the user saying it a second time.
    const first = new Date(T0).toISOString();
    expect(backupReminderNextDismissal(first, T0 + 31 * DAY_MS)).toBe(BACKUP_REMINDER_FOREVER);
    expect(backupReminderNextDismissal(BACKUP_REMINDER_FOREVER, T0)).toBe(BACKUP_REMINDER_FOREVER);
  });

  it('backup being on beats any dismissal state', () => {
    expect(backupReminderVisibleIn({}, T0)).toBe(true);
    expect(backupReminderVisibleIn(BACKUP_ON, T0)).toBe(false);
    expect(backupReminderVisibleIn({ ...BACKUP_ON, backup_reminder_dismissed: '' }, T0)).toBe(false);
    // Switched back off after a dismissal whose snooze has since lapsed.
    expect(backupReminderVisibleIn({ backup_reminder_dismissed: new Date(T0).toISOString() }, T0 + 31 * DAY_MS)).toBe(true);
  });

  it('fails closed while preferences are not loaded (no flash for returning users)', () => {
    // Same reasoning as welcomeSuggestionsDismissed above, and it matters more
    // on an iOS PWA, where the preferences fetch reruns on every resume.
    for (const state of [
      { status: 'not-loaded' } as const,
      { status: 'loading' } as const,
      { status: 'failed', error: 'network error' } as const,
    ]) {
      preferences.value = state;
      expect(backupReminderVisible(T0)).toBe(false);
    }
    preferences.value = { status: 'loaded', data: {} };
    expect(backupReminderVisible(T0)).toBe(true);
  });

  it('dismissBackupReminder writes the instant, then forever', async () => {
    preferences.value = { status: 'loaded', data: {} };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);

    await dismissBackupReminder(T0);
    expect(spy).toHaveBeenCalledWith('backup_reminder_dismissed', new Date(T0).toISOString(), undefined);

    // savePreference updates the local map optimistically, so the second
    // dismissal reads the first one's value and escalates.
    await dismissBackupReminder(T0 + 31 * DAY_MS);
    expect(spy).toHaveBeenLastCalledWith('backup_reminder_dismissed', BACKUP_REMINDER_FOREVER, undefined);
  });

  it('dismissBackupReminder is a no-op while preferences are unloaded', async () => {
    preferences.value = { status: 'not-loaded' };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);

    await dismissBackupReminder(T0);

    expect(spy).not.toHaveBeenCalled();
  });
});

describe('currentInAppBrowser — experimental in-app browser, off by default', () => {
  beforeEach(() => {
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('is off while preferences are not loaded (fails safe to the system browser)', () => {
    expect(currentInAppBrowser()).toBe(false);
  });

  it('is off on a loaded workspace with the preference unset', () => {
    preferences.value = { status: 'loaded', data: {} };
    expect(currentInAppBrowser()).toBe(false);
  });

  it('is on only when explicitly set to "true"', () => {
    preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };
    expect(currentInAppBrowser()).toBe(true);
  });

  it('treats any non-"true" value as off', () => {
    preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'false' } };
    expect(currentInAppBrowser()).toBe(false);
  });

  it('setInAppBrowser persists the boolean as a string preference', async () => {
    preferences.value = { status: 'loaded', data: {} };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);

    await setInAppBrowser(true);

    expect(spy).toHaveBeenCalledWith('experimental_in_app_browser', 'true', undefined);
    expect((preferences.value as { data: Record<string, string> }).data.experimental_in_app_browser).toBe('true');
  });
});

/**
 * The surfaces that must agree about the in-app browser being the live URL
 * target: the menu drawer's Browser row (its only entry point) and
 * `restoreState`'s refusal to resurrect a url-preview overlay on reload.
 *
 * The preference half is the one that shipped missing from the row. With the
 * toggle off `openUrl` deliberately routes to the OS opener, so the row rendered
 * for every desktop user and a menu entry labelled "Browser" just launched the
 * system browser on google.com.
 */
describe('inAppBrowserAvailable: desktop app AND the experimental opt-in', () => {
  beforeEach(() => {
    platformMocks.isTauri = false;
    preferences.value = { status: 'loaded', data: {} };
  });

  it('is off in the desktop app while the experimental toggle is off', () => {
    platformMocks.isTauri = true;
    expect(inAppBrowserAvailable()).toBe(false);
  });

  it('is on in the desktop app once the experimental toggle is on', () => {
    platformMocks.isTauri = true;
    preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };
    expect(inAppBrowserAvailable()).toBe(true);
  });

  it('is off on web/PWA even with the toggle on, where there is no native webview', () => {
    preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };
    expect(inAppBrowserAvailable()).toBe(false);
  });

  it('is off while preferences are still loading, so the row cannot flash in', () => {
    platformMocks.isTauri = true;
    preferences.value = { status: 'loading' };
    expect(inAppBrowserAvailable()).toBe(false);
  });
});

/**
 * The external-link target is read on the tap itself, inside a code path with
 * no `await` before it (the Web Share branch needs the user activation intact),
 * so every fallback has to be a pure synchronous default rather than a retry.
 * That is what these cases pin: unset, still-loading, and a value the enum
 * doesn't know all resolve to `safari`, the behaviour shipped in dbc7386d.
 */
describe('currentExternalLinkTarget: safari unless the user chose otherwise', () => {
  beforeEach(() => {
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('defaults to safari on a loaded workspace with the preference unset', () => {
    preferences.value = { status: 'loaded', data: {} };
    expect(currentExternalLinkTarget()).toBe('safari');
  });

  it('defaults to safari while preferences are still loading', () => {
    // A link tapped during startup must not land in a different mode than the
    // same link tapped a second later.
    preferences.value = { status: 'loading' };
    expect(currentExternalLinkTarget()).toBe('safari');
  });

  it('returns each of the three modes the user can choose', () => {
    for (const target of ['safari', 'ask', 'in-app'] as const) {
      preferences.value = { status: 'loaded', data: { external_link_target: target } };
      expect(currentExternalLinkTarget()).toBe(target);
    }
  });

  it('degrades an unrecognized stored value to safari, never to no hand-off', () => {
    // e.g. a value written by a newer build, or a hand-edited row. Falling back
    // to the working default beats leaving the user trapped in the web view.
    preferences.value = { status: 'loaded', data: { external_link_target: 'chrome' } };
    expect(currentExternalLinkTarget()).toBe('safari');
  });

  it('setExternalLinkTarget persists the chosen mode', async () => {
    preferences.value = { status: 'loaded', data: {} };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);

    await setExternalLinkTarget('ask');

    expect(spy).toHaveBeenCalledWith('external_link_target', 'ask', undefined);
    expect((preferences.value as { data: Record<string, string> }).data.external_link_target).toBe('ask');
  });
});

/**
 * The Settings row must not render where the choice decides nothing. Every
 * client except an installed iOS PWA opens a new tab (or the desktop OS opener)
 * regardless of the stored value, so a row there would be a control that does
 * nothing, which `.claude/rules/frontend.md` treats as a lie rather than a
 * harmless extra.
 */
describe('externalLinkTargetConfigurable: the row shows only where it bites', () => {
  beforeEach(() => {
    platformMocks.isIOSPwa = false;
    platformMocks.isTauri = false;
    preferences.value = { status: 'loaded', data: {} };
  });

  it('is on for an installed iOS PWA', () => {
    platformMocks.isIOSPwa = true;
    expect(externalLinkTargetConfigurable()).toBe(true);
  });

  it('is off in a desktop browser and a normal mobile Safari tab', () => {
    expect(externalLinkTargetConfigurable()).toBe(false);
  });

  it('is off in the desktop app, which has its own in-app browser toggle', () => {
    platformMocks.isTauri = true;
    expect(externalLinkTargetConfigurable()).toBe(false);
  });

  it('does not depend on preferences having loaded', () => {
    // Platform-only, so the row cannot pop in partway through startup.
    platformMocks.isIOSPwa = true;
    preferences.value = { status: 'loading' };
    expect(externalLinkTargetConfigurable()).toBe(true);
  });
});

/**
 * An installed iOS PWA suspends tens of times a day, and WebKit aborts every
 * in-flight fetch when it does. `savePreference` applies the value locally
 * first, so the only thing left to go wrong is delivery, and it used to go
 * wrong loudly and permanently: one toast per cancelled write, never retried,
 * leaving the device showing a value the server never received.
 *
 * The contract now: a transient rejection is retried, then parked and flushed
 * on resume, silently; a real engine verdict still speaks up at once; and
 * neither can stack more than one card.
 */
describe('preference writes survive an iOS PWA suspend', () => {
  /** What WebKit rejects with when it kills an in-flight fetch. */
  const cancelled = () => new DOMException('Fetch is aborted', 'AbortError');

  beforeEach(() => {
    vi.restoreAllMocks();
    _resetPendingPreferenceWritesForTesting();
    toasts.value = [];
    preferences.value = { status: 'loaded', data: {} };
  });

  afterEach(() => {
    _resetPendingPreferenceWritesForTesting();
    toasts.value = [];
  });

  it('retries once immediately and stays quiet when the second attempt lands', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference')
      .mockRejectedValueOnce(cancelled())
      .mockResolvedValueOnce(undefined as never);

    await savePreference('theme', 'light');

    expect(spy).toHaveBeenCalledTimes(2);
    expect(toasts.value).toHaveLength(0);
    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
  });

  it('parks the write with no toast when both attempts are cancelled', async () => {
    vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());

    await savePreference('ui-scale', '125', undefined, true);

    expect(toasts.value).toHaveLength(0);
    expect(_pendingPreferenceKeysForTesting()).toEqual(['ui-scale']);
  });

  it('re-sends parked writes on resume, then clears them', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());
    await savePreference('ui-scale', '125', undefined, true);
    expect(_pendingPreferenceKeysForTesting()).toEqual(['ui-scale']);

    spy.mockResolvedValue(undefined as never);
    await flushPendingPreferenceWrites();

    expect(spy).toHaveBeenLastCalledWith('ui-scale', '125', expect.any(String));
    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
  });

  it('last write wins per key: the superseded value is dropped, not queued behind it', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());
    await savePreference('ui-scale', '112.5', undefined, true);
    await savePreference('ui-scale', '150', undefined, true);
    expect(_pendingPreferenceKeysForTesting()).toEqual(['ui-scale']);

    spy.mockReset();
    spy.mockResolvedValue(undefined as never);
    await flushPendingPreferenceWrites();

    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith('ui-scale', '150', expect.any(String));
  });

  it('surfaces a real engine rejection at once, with no retry and nothing parked', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference')
      .mockRejectedValue(new ApiError(400, 'unknown preference key'));

    await savePreference('bogus', 'x');

    expect(spy).toHaveBeenCalledTimes(1);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('unknown preference key');
    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
  });

  it('collapses repeated rejections into one card instead of stacking', async () => {
    vi.spyOn(apiClient, 'setPreference').mockRejectedValue(new ApiError(500, 'db down'));

    await savePreference('theme', 'light');
    await savePreference('font-family', 'inter');
    await savePreference('ui-scale', '125', undefined, true);

    expect(toasts.value).toHaveLength(1);
  });

  it('speaks once when writes keep failing to arrive, naming what is stuck', async () => {
    vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());

    await savePreference('theme', 'light');
    expect(toasts.value).toHaveLength(0);
    await savePreference('font-family', 'inter');
    expect(toasts.value).toHaveLength(0);

    await savePreference('ui-scale', '125', undefined, true);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('font-family, theme, ui-scale');
  });

  it('retracts the unreachable banner once the queue drains', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());
    await savePreference('theme', 'light');
    await savePreference('font-family', 'inter');
    await savePreference('ui-scale', '125', undefined, true);
    expect(toasts.value).toHaveLength(1);

    spy.mockResolvedValue(undefined as never);
    await flushPendingPreferenceWrites();

    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
    expect(toasts.value).toHaveLength(0);
  });

  it('applies the value locally before the network call, so a failed write still shows', async () => {
    vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());
    const applied = vi.fn();

    await savePreference('theme', 'light', applied);

    expect(applied).toHaveBeenCalledTimes(1);
    expect(preferences.value).toMatchObject({ data: { theme: 'light' } });
  });
});

/**
 * `PUT /preferences?key=<k>` is applied in ARRIVAL order, so two overlapping
 * writes to one key are a lost-update race the local bookkeeping cannot see:
 * an older request landing second puts the stale value on the server while the
 * device shows the newer one. Deliveries are therefore serialized per key, and
 * a write that a newer one has superseded stands down instead of sending.
 */
describe('concurrent writes to one preference key', () => {
  const cancelled = () => new DOMException('Fetch is aborted', 'AbortError');

  beforeEach(() => {
    vi.restoreAllMocks();
    _resetPendingPreferenceWritesForTesting();
    toasts.value = [];
    preferences.value = { status: 'loaded', data: {} };
  });

  afterEach(() => {
    _resetPendingPreferenceWritesForTesting();
    toasts.value = [];
  });

  it('never has two writes for one key in flight at once', async () => {
    let inFlight = 0;
    let maxInFlight = 0;
    vi.spyOn(apiClient, 'setPreference').mockImplementation(async () => {
      maxInFlight = Math.max(maxInFlight, ++inFlight);
      await Promise.resolve();
      inFlight--;
      return { success: true };
    });

    await Promise.all([
      savePreference('ui-scale', '112.5', undefined, true),
      savePreference('ui-scale', '125', undefined, true),
      savePreference('ui-scale', '150', undefined, true),
    ]);

    expect(maxInFlight).toBe(1);
  });

  it('drops a write superseded before it was ever dispatched', async () => {
    const sent: string[] = [];
    vi.spyOn(apiClient, 'setPreference').mockImplementation(async (_k, v) => {
      sent.push(v);
      return { success: true };
    });

    // Both requested in the same tick, so the first has not gone out yet when
    // the second claims the key. Sending it at all would be pure waste.
    await Promise.all([
      savePreference('ui-scale', '112.5', undefined, true),
      savePreference('ui-scale', '150', undefined, true),
    ]);

    expect(sent).toEqual(['150']);
    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
  });

  it('does not re-send a superseded write between its own two attempts', async () => {
    const sent: string[] = [];
    let failFirstAttempt: (() => void) | null = null;
    let markDispatched: () => void = () => {};
    const firstDispatched = new Promise<void>(r => { markDispatched = r; });

    vi.spyOn(apiClient, 'setPreference').mockImplementation((_k, v) => {
      sent.push(v);
      // Hold the very first request open so the test can interleave a newer
      // write while it is genuinely on the wire.
      if (v === '112.5' && !failFirstAttempt) {
        return new Promise<ApiResult>((_resolve, reject) => {
          failFirstAttempt = () => reject(cancelled());
          markDispatched();
        });
      }
      return Promise.reject(cancelled());
    });

    const first = savePreference('ui-scale', '112.5', undefined, true);
    await firstDispatched;
    const second = savePreference('ui-scale', '150', undefined, true);
    failFirstAttempt!();
    await Promise.all([first, second]);

    // The older value went out once and was NOT retried: by then the user had
    // moved on, and its retry would have landed after the newer write.
    expect(sent.filter(v => v === '112.5')).toHaveLength(1);
    expect(sent.filter(v => v === '150')).toHaveLength(2);
    expect(_pendingPreferenceKeysForTesting()).toEqual(['ui-scale']);
  });

  it('keeps a newer parked write when an older one is rejected outright', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference');
    // Older write: two cancels, so it parks.
    spy.mockRejectedValue(cancelled());
    await savePreference('ui-scale', '125', undefined, true);
    expect(_pendingPreferenceKeysForTesting()).toEqual(['ui-scale']);

    // A LATER rejection for the same key must not evict the parked value: it is
    // still the user's choice and still owed a re-send.
    spy.mockRejectedValue(new ApiError(500, 'db down'));
    await flushPendingPreferenceWrites();
    expect(toasts.value.some(t => t.message.includes('db down'))).toBe(true);
    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
  });

  it('writes to different keys still go out in parallel', async () => {
    let inFlight = 0;
    let maxInFlight = 0;
    vi.spyOn(apiClient, 'setPreference').mockImplementation(async () => {
      maxInFlight = Math.max(maxInFlight, ++inFlight);
      await Promise.resolve();
      inFlight--;
      return { success: true };
    });

    await Promise.all([
      savePreference('theme', 'light'),
      savePreference('font-family', 'inter'),
      savePreference('ui-scale', '125', undefined, true),
    ]);

    expect(maxInFlight).toBeGreaterThan(1);
  });
});

/**
 * The two failure banners answer different questions, so draining the queue must
 * retract the "not reaching the engine" one whichever way it drained. A queue
 * emptied by rejections used to leave it on screen insisting nothing was getting
 * through, directly contradicting the rejection card beside it.
 */
describe('unreachable banner tracks the queue, not just the happy path', () => {
  const cancelled = () => new DOMException('Fetch is aborted', 'AbortError');
  const UNREACHABLE = 'preference-save-unreachable';

  beforeEach(() => {
    vi.restoreAllMocks();
    _resetPendingPreferenceWritesForTesting();
    toasts.value = [];
    preferences.value = { status: 'loaded', data: {} };
  });

  afterEach(() => {
    _resetPendingPreferenceWritesForTesting();
    toasts.value = [];
  });

  /** Park three writes so the escalation threshold trips. */
  async function stallThreeWrites() {
    const spy = vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());
    await savePreference('theme', 'light');
    await savePreference('font-family', 'inter');
    await savePreference('ui-scale', '125', undefined, true);
    expect(toasts.value.some(t => t.key === UNREACHABLE)).toBe(true);
    return spy;
  }

  it('retracts it when the queue drains through rejections, not just successes', async () => {
    const spy = await stallThreeWrites();

    spy.mockRejectedValue(new ApiError(400, 'unknown preference key'));
    await flushPendingPreferenceWrites();

    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
    expect(toasts.value.some(t => t.key === UNREACHABLE)).toBe(false);
    // The rejection itself still has to be readable.
    expect(toasts.value.some(t => t.message.includes('unknown preference key'))).toBe(true);
  });

  it('treats a rejection as proof the engine is reachable, resetting the count', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference');
    spy.mockRejectedValue(cancelled());
    await savePreference('theme', 'light');
    await savePreference('font-family', 'inter');

    // An answer, even a refusal, breaks the "nothing is getting through" streak.
    spy.mockRejectedValue(new ApiError(400, 'nope'));
    await savePreference('image_model', 'auto');
    toasts.value = toasts.value.filter(t => t.key !== 'preference-save-rejected');

    // So the next single cancel is back to being noise, not the third strike.
    spy.mockRejectedValue(cancelled());
    await savePreference('timezone', 'UTC');
    expect(toasts.value.some(t => t.key === UNREACHABLE)).toBe(false);
  });

  it('a successful save does not churn the toast signal when nothing is showing', async () => {
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });
    const before = toasts.value;

    await savePreference('theme', 'light');

    // Same array identity: retracting an absent banner must not notify
    // subscribers, or every preference save re-renders the toast container.
    expect(toasts.value).toBe(before);
  });
});


/**
 * `currentMaxToolCalls` mirrors `PreferenceStore::max_tool_calls` in
 * `core/preferences.rs`. The two must agree, or Settings shows a cap the engine
 * would not honor: the user reads 12 and the loop runs 500.
 *
 * The engine parses with `parse::<usize>()`, which is stricter than JS's
 * `parseInt` in exactly the ways that matter here, so these cases are the
 * contract between them rather than incidental input validation.
 */
describe('currentMaxToolCalls: mirrors the engine resolution', () => {
  beforeEach(() => {
    preferences.value = { status: 'loaded', data: {} };
  });

  it('defaults when unset, so an untouched workspace reads as it behaves', () => {
    expect(currentMaxToolCalls()).toBe(MAX_TOOL_CALLS_DEFAULT);
  });

  it('defaults while preferences are still loading', () => {
    preferences.value = { status: 'loading' };
    expect(currentMaxToolCalls()).toBe(MAX_TOOL_CALLS_DEFAULT);
  });

  it('honors a stored value, including one far above any preset', () => {
    preferences.value = { status: 'loaded', data: { max_tool_calls: '2000' } };
    expect(currentMaxToolCalls()).toBe(2000);
    // There is no ceiling: a huge cap is the user's call to make, and the UI
    // must show what is actually stored rather than a clamped fiction.
    preferences.value = { status: 'loaded', data: { max_tool_calls: '1000000' } };
    expect(currentMaxToolCalls()).toBe(1_000_000);
  });

  it('tolerates surrounding whitespace, as the engine does', () => {
    preferences.value = { status: 'loaded', data: { max_tool_calls: ' 750 ' } };
    expect(currentMaxToolCalls()).toBe(750);
  });

  it('raises 0 to the floor, the one value that would break the turn', () => {
    // The loop checks `iterations > cap` after incrementing, so 0 ends the turn
    // before the first LLM call.
    preferences.value = { status: 'loaded', data: { max_tool_calls: '0' } };
    expect(currentMaxToolCalls()).toBe(MAX_TOOL_CALLS_MIN);
  });

  it('shows the representable bound rather than a silently rounded number', () => {
    // Only reachable by a write from outside this UI (CLI / HTTP / psql), since
    // setMaxToolCalls refuses these. `Number('1'.repeat(400))` is Infinity and
    // a 20-digit value rounds, either of which would render a figure the engine
    // is not enforcing.
    preferences.value = { status: 'loaded', data: { max_tool_calls: '1'.repeat(400) } };
    expect(currentMaxToolCalls()).toBe(Number.MAX_SAFE_INTEGER);
    preferences.value = { status: 'loaded', data: { max_tool_calls: '99999999999999999999' } };
    expect(currentMaxToolCalls()).toBe(Number.MAX_SAFE_INTEGER);
  });

  it('falls back to the default for anything that is not a whole number', () => {
    // "-5" and "12.5" are the cases where a bare parseInt would diverge from the
    // engine's usize parse, reading -5 and 12 where the engine reads neither.
    for (const raw of ['', '   ', 'abc', '-5', '12.5', '1e3', '1_000']) {
      preferences.value = { status: 'loaded', data: { max_tool_calls: raw } };
      expect(currentMaxToolCalls(), `stored ${JSON.stringify(raw)}`).toBe(MAX_TOOL_CALLS_DEFAULT);
    }
  });
});

/**
 * The Settings note tells the user what a cap means in wall-clock terms before
 * they pick one, which is the whole reason there is no maximum: the number is
 * theirs to choose, so it has to be legible. Coarse on purpose.
 */
describe('estimateTurnDuration', () => {
  it('scales from minutes through hours to days', () => {
    expect(estimateTurnDuration(50)).toBe('13 min');
    expect(estimateTurnDuration(500)).toBe('2.1 hours');
    expect(estimateTurnDuration(2000)).toBe('8.3 hours');
    expect(estimateTurnDuration(100000)).toBe('17 days');
  });

  it('never reads as zero for the smallest allowed cap', () => {
    expect(estimateTurnDuration(MAX_TOOL_CALLS_MIN)).toBe('1 min');
  });
});