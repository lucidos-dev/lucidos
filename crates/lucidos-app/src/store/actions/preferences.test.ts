import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { preferences, toasts, llmConfigured, selectedCodingAgent, threadMap, lastSuccessfulBackup } from '../store';
import { makeThreadState } from './threads-test-helpers';
import { getComposeSelectionOverride, resolveCodingAgent, _resetComposeSelectionsForTesting } from '../composeSelections';
import { preferencesSlice, currentAgentBrandColours, setAgentBrandColours, applyStyleOverrides, currentComposerEnterKey, setComposerEnterKey } from './preferences';
import { PREF_COMPOSER_ENTER_KEY } from '@lucidos/preference-catalog';
import { applyThemeMode, applyFontFamily, applyUiScale, currentThemeMode, currentFontFamily, refreshActiveTheme, loadPreferences, currentMobileDynamicBars, setMobileDynamicBars, welcomeSuggestionsDismissed, dismissWelcomeSuggestions, retireWelcomeAfterUse, WELCOME_RETIRES_AFTER_THREADS, currentInAppBrowser, setInAppBrowser, inAppBrowserAvailable, currentExternalLinkTarget, setExternalLinkTarget, externalLinkTargetConfigurable, savePreference, flushPendingPreferenceWrites, _pendingPreferenceKeysForTesting, _resetPendingPreferenceWritesForTesting, currentMaxToolCalls, estimateTurnDuration, MAX_TOOL_CALLS_DEFAULT, MAX_TOOL_CALLS_MAX, MAX_TOOL_CALLS_MIN, backupIsConfigured, backupReminderHiddenByDismissal, backupReminderNextDismissal, backupReminderVisibleIn, backupBanner, backupBannerIn, backupOverdueHiddenByDismissal, dismissBackupOverdueReminder, BACKUP_OVERDUE_SNOOZE_MS, dismissBackupReminder, BACKUP_REMINDER_FOREVER, BACKUP_REMINDER_SNOOZE_MS, currentNotificationToasts, setNotificationToasts, VOICE_RESIDENT_SECTIONS, DEFAULT_VOICE_RESIDENT_SECTIONS, voiceSectionEnabled, setVoiceSectionEnabled, storedBackgroundSelection, currentAutocorrect, setAutocorrect, currentMotion, setMotion, currentMemoryModule, setMemoryModule, storedMemoryModule } from './preferences';
import { motionPreference } from '../../utils/motion';
import { MEMORY_MODULE_WHILE_UNSET } from '@lucidos/engine-constants';
import { SYSTEM_THEME_MODE_CONFIRM_MS, SYSTEM_THEME_MODE_SETTLE_MS, THEME_MODE_HEADER_TOP } from '@lucidos/appearance';
import * as apiClient from '../../api/client';
import { ApiError } from '../../api/client';
import type { ApiResult } from '../../api/types';
import type { ThreadState } from '../thread-events';
import type { Loadable } from '../types';

const platformMocks = vi.hoisted(() => ({ isTauri: false, isIOSPwa: false, isIOS: false }));
vi.mock('../../utils/platform', () => ({
  isTauri: () => platformMocks.isTauri,
  isIOSPwa: () => platformMocks.isIOSPwa,
  isIOS: () => platformMocks.isIOS,
}));

const setProseAutocorrectMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/noAutofill', () => ({ setProseAutocorrect: setProseAutocorrectMock }));

const setTitlebarColorMock = vi.hoisted(() => vi.fn(() => Promise.resolve()));
const windowReadyToShowMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/tauri', () => ({
  setTitlebarColor: setTitlebarColorMock,
  windowReadyToShow: windowReadyToShowMock,
}));

const resolvedTokenHexMock = vi.hoisted(() => vi.fn((_name: string): string | null => null));
vi.mock('../../utils/cssColor', () => ({ resolvedTokenHex: resolvedTokenHexMock }));

describe('currentThemeMode: localStorage fallback', () => {
  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  it('returns localStorage theme when backend has no theme preference', () => {
    localStorage.setItem('lucidos-theme-mode', 'light');
    preferences.value = { status: 'loaded', data: { 'font-family': 'monospace' } };

    expect(currentThemeMode()).toBe('light');
  });

  it('returns backend theme when backend has theme preference', () => {
    localStorage.setItem('lucidos-theme-mode', 'light');
    preferences.value = { status: 'loaded', data: { 'theme-mode': 'dark' } };

    expect(currentThemeMode()).toBe('dark');
  });

  it('returns localStorage theme when preferences not yet loaded', () => {
    localStorage.setItem('lucidos-theme-mode', 'light');
    preferences.value = { status: 'loading' };

    expect(currentThemeMode()).toBe('light');
  });

  it('follows the OS as final fallback when nothing is set', () => {
    preferences.value = { status: 'loaded', data: {} };

    expect(currentThemeMode()).toBe('system');
  });

  it('returns system from localStorage when backend has no theme', () => {
    localStorage.setItem('lucidos-theme-mode', 'system');
    preferences.value = { status: 'loaded', data: {} };

    expect(currentThemeMode()).toBe('system');
  });

  it('returns localStorage theme when preferences failed to load', () => {
    localStorage.setItem('lucidos-theme-mode', 'light');
    preferences.value = { status: 'failed', error: 'network error' };

    expect(currentThemeMode()).toBe('light');
  });

  it('skips invalid backend value and falls back to localStorage', () => {
    localStorage.setItem('lucidos-theme-mode', 'light');
    preferences.value = { status: 'loaded', data: { 'theme-mode': 'garbage' } };

    expect(currentThemeMode()).toBe('light');
  });

  it('ignores invalid localStorage values', () => {
    localStorage.setItem('lucidos-theme-mode', 'purple');
    preferences.value = { status: 'loaded', data: {} };

    expect(currentThemeMode()).toBe('system');
  });

  it('defaults an untouched device to system theme and following the theme', () => {
    preferences.value = { status: 'loaded', data: {} };

    expect(currentThemeMode()).toBe('system');
    expect(currentFontFamily()).toBe('theme');
  });

  it('a stored font pick survives the default', () => {
    preferences.value = { status: 'loaded', data: { 'font-family': 'ibm-plex-mono' } };

    expect(currentFontFamily()).toBe('ibm-plex-mono');
  });
});

describe('apply* functions mirror to localStorage for FOUC inline script', () => {

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

  it('applyThemeMode writes lucidos-theme-mode to localStorage', () => {
    applyThemeMode('light');
    expect(localStorage.getItem('lucidos-theme-mode')).toBe('light');
    expect(attrs['data-theme-mode']).toBe('light');
    expect(inlineProps['--bg-primary']).toBe('#ffffff');
  });

  it('applyThemeMode keeps inline --bg-primary in sync on toggle', () => {
    applyThemeMode('light');
    expect(inlineProps['--bg-primary']).toBe('#ffffff');
    applyThemeMode('dark');
    expect(inlineProps['--bg-primary']).toBe('#07172e');
  });

  it('applyThemeMode sets html.style.background inline so the WebView has a paintable bg before global.css loads', () => {
    const el = (document as any).documentElement;
    applyThemeMode('dark');
    expect(el.style.background).toBe('#07172e');
    applyThemeMode('light');
    expect(el.style.background).toBe('#ffffff');
  });

  it('applyFontFamily writes lucidos-font-family to localStorage', () => {
    applyFontFamily('monospace');
    expect(localStorage.getItem('lucidos-font-family')).toBe('monospace');
    expect(inlineProps['--font-ui']).toContain('SF Mono');
  });

  describe('the font a theme suggests (theme suggests, user wins)', () => {
    const FIRA = "'Fira Code', ui-monospace,";

    async function activateTheme(fonts: Record<string, string>): Promise<void> {
      vi.spyOn(apiClient, 'getTheme').mockResolvedValue({
        id: 'harbour', source: 'workspace', name: 'Harbour', modes: [],
        resolved: { dark: {}, light: {}, fonts },
      } as any);
      await refreshActiveTheme('harbour');
    }

    afterEach(async () => {
      await refreshActiveTheme('lucidos');
      vi.restoreAllMocks();
    });

    it('a device that follows the theme paints the theme font', async () => {
      preferences.value = { status: 'loaded', data: {} };
      await activateTheme({ ui: 'geist' });
      expect(inlineProps['--font-ui']).toMatch(/^'Geist',/);
      expect(localStorage.getItem('lucidos-font-family')).toBe('theme');
    });

    it('an explicit pick wins over the theme font', async () => {
      preferences.value = { status: 'loaded', data: { 'font-family': 'ibm-plex-mono' } };
      await activateTheme({ ui: 'geist' });
      expect(inlineProps['--font-ui']).toMatch(/^'IBM Plex Mono', /);
      expect(localStorage.getItem('lucidos-font-family')).toBe('ibm-plex-mono');
    });

    it('a theme with no font falls back to Fira Code', async () => {
      preferences.value = { status: 'loaded', data: {} };
      await activateTheme({});
      expect(inlineProps['--font-ui'].startsWith(FIRA)).toBe(true);
    });

    it('switching back to the default theme drops the theme font', async () => {
      preferences.value = { status: 'loaded', data: {} };
      await activateTheme({ ui: 'geist' });
      await refreshActiveTheme('lucidos');
      expect(inlineProps['--font-ui'].startsWith(FIRA)).toBe(true);
    });
  });

  it('applyFontFamily turns Fira Code ligatures OFF for text with explicit zeros, not `normal`', () => {
    applyFontFamily('fira-code');
    expect(inlineProps['--font-features-text']).toBe('"liga" 0, "calt" 0');
    expect(inlineProps['font-feature-settings']).toBeUndefined();
  });

  it('applyFontFamily turns them back ON for code', () => {
    applyFontFamily('fira-code');
    expect(inlineProps['--font-features-code']).toBe('"liga" 1, "calt" 1');
  });

  it('applyFontFamily resolves BOTH properties to normal for every other font', () => {
    applyFontFamily('fira-code');
    applyFontFamily('monospace');
    expect(inlineProps['--font-features-text']).toBe('normal');
    expect(inlineProps['--font-features-code']).toBe('normal');
  });

  it('applyFontFamily says on <html> whether the UI font has a bold face', () => {
    applyFontFamily('vt323');
    expect(document.documentElement.getAttribute('data-font-bold')).toBe('none');
    applyFontFamily('fira-code');
    expect(document.documentElement.getAttribute('data-font-bold')).toBe('face');
  });

  it('applyUiScale writes lucidos-ui-scale to localStorage', () => {
    applyUiScale(125);
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('125');
    expect(inlineProps['--user-ui-scale']).toBe('125%');
  });

  it('applyUiScale clamps and stores the clamped value', () => {
    applyUiScale(500);
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

  it('establishes the saved agent on startup before preserving drafts on later refreshes', async () => {
    const previousThreads = threadMap.value;
    threadMap.value = new Map([['legacy', makeThreadState('legacy', { meta: { state: 'composing' } })]]);
    selectedCodingAgent.value = 'claude-code';
    const read = vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: { coding_agent_default: 'codex' } });
    try {
      await loadPreferences();
      expect(resolveCodingAgent('legacy')).toBe('codex');
      expect(getComposeSelectionOverride('legacy').codingAgent).toBeUndefined();
      read.mockResolvedValue({ preferences: { coding_agent_default: 'claude-code' } });
      await loadPreferences();
      expect(resolveCodingAgent('legacy')).toBe('codex');
      expect(selectedCodingAgent.value).toBe('claude-code');
    } finally {
      threadMap.value = previousThreads;
      selectedCodingAgent.value = 'claude-code';
      _resetComposeSelectionsForTesting();
    }
  });

  it('does not wipe loaded state to "loading" on a refetch', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({
      preferences: { 'theme-mode': 'dark', 'font-family': 'monospace' },
    });
    await loadPreferences();
    expect(preferences.value.status).toBe('loaded');

    let observedLoadingDuringRefetch = false;
    const reloadPromise = (async () => {
      const promise = loadPreferences();
      if ((preferences.value as { status: string }).status === 'loading') {
        observedLoadingDuringRefetch = true;
      }
      await promise;
    })();
    await reloadPromise;

    expect(observedLoadingDuringRefetch).toBe(false);
    expect(preferences.value.status).toBe('loaded');
  });

  it('a superseded call resolves only once the newest call has loaded', async () => {
    const replies: Array<(v: { preferences: Record<string, string> }) => void> = [];
    vi.spyOn(apiClient, 'getPreferences').mockImplementation(
      () => new Promise(resolve => { replies.push(resolve); }),
    );
    let statusWhenStartupResumed: string | null = null;
    const startup = loadPreferences().then(() => { statusWhenStartupResumed = preferences.value.status; });
    const onOpen = loadPreferences();
    await vi.waitFor(() => expect(replies).toHaveLength(2));

    replies[0]({ preferences: { 'theme-mode': 'light' } });
    await Promise.resolve();
    expect(statusWhenStartupResumed).toBeNull();

    replies[1]({ preferences: { 'theme-mode': 'dark' } });
    await Promise.all([startup, onOpen]);
    expect(statusWhenStartupResumed).toBe('loaded');
    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'dark' } });
  });

  it('a call superseded by a snapshot read resolves once the snapshot has applied', async () => {
    const replies: Array<(v: { preferences: Record<string, string> }) => void> = [];
    vi.spyOn(apiClient, 'getPreferences').mockImplementation(
      () => new Promise(resolve => { replies.push(resolve); }),
    );
    let statusWhenNarrowResumed: string | null = null;
    const narrow = loadPreferences().then(() => { statusWhenNarrowResumed = preferences.value.status; });
    await vi.waitFor(() => expect(replies).toHaveLength(1));
    const snapshotRead = preferencesSlice.begin();

    replies[0]({ preferences: { 'theme-mode': 'light' } });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(statusWhenNarrowResumed).toBeNull();
    expect(preferences.value.status).toBe('loading');

    preferencesSlice.apply({ preferences: { 'theme-mode': 'dark' } }, snapshotRead);
    await narrow;
    expect(statusWhenNarrowResumed).toBe('loaded');
    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'dark' } });
  });

  it('a call superseded by a snapshot read resolves when that read fails too', async () => {
    const replies: Array<(v: { preferences: Record<string, string> }) => void> = [];
    vi.spyOn(apiClient, 'getPreferences').mockImplementation(
      () => new Promise(resolve => { replies.push(resolve); }),
    );
    const narrow = loadPreferences();
    await vi.waitFor(() => expect(replies).toHaveLength(1));
    const snapshotRead = preferencesSlice.begin();
    replies[0]({ preferences: { 'theme-mode': 'light' } });

    preferencesSlice.fail(new Error('offline'), snapshotRead);
    await narrow;
    expect(preferences.value.status).toBe('failed');
  });

  it('still flips to "loading" on the very first call', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockImplementation(async () => {
      return { preferences: { 'theme-mode': 'dark' } };
    });

    const promise = loadPreferences();
    expect(preferences.value.status).toBe('loading');
    await promise;
    expect(preferences.value.status).toBe('loaded');
  });
});

describe('loadPreferences: transient failures retry, real ones surface', () => {
  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('retries once when the browser cancels the fetch mid-flight', async () => {
    vi.spyOn(apiClient, 'getPreferences')
      .mockRejectedValueOnce(new DOMException('Fetch is aborted', 'AbortError'))
      .mockResolvedValueOnce({ preferences: { 'theme-mode': 'dark' } });

    await loadPreferences();

    expect(apiClient.getPreferences).toHaveBeenCalledTimes(2);
    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'dark' } });
  });

  it('retries once when our own deadline fires (engine still booting)', async () => {
    vi.spyOn(apiClient, 'getPreferences')
      .mockRejectedValueOnce(new DOMException('Request timed out', 'TimeoutError'))
      .mockResolvedValueOnce({ preferences: {} });

    await loadPreferences();

    expect(apiClient.getPreferences).toHaveBeenCalledTimes(2);
    expect(preferences.value.status).toBe('loaded');
  });

  it('does NOT retry a real backend failure, and keeps the engine reason', async () => {
    vi.spyOn(apiClient, 'getPreferences')
      .mockRejectedValue(new ApiError(500, 'Failed to load preferences: DB error'));

    await loadPreferences();

    expect(apiClient.getPreferences).toHaveBeenCalledTimes(1);
    expect(preferences.value).toMatchObject({
      status: 'failed',
      error: 'Failed to load preferences: DB error',
    });
  });

  it('parks on failed when both attempts are cancelled', async () => {
    vi.spyOn(apiClient, 'getPreferences')
      .mockRejectedValue(new DOMException('Fetch is aborted', 'AbortError'));

    await loadPreferences();

    expect(apiClient.getPreferences).toHaveBeenCalledTimes(2);
    expect(preferences.value.status).toBe('failed');
  });
});

describe('loadPreferences: a stale response cannot overwrite a fresher one', () => {
  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('both calls issue their own fetch rather than sharing one', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: {} });

    await Promise.all([loadPreferences(), loadPreferences()]);

    expect(apiClient.getPreferences).toHaveBeenCalledTimes(2);
  });

  it('an older response landing after a newer one is discarded', async () => {
    let resolveOlder!: (v: { preferences: Record<string, string> }) => void;
    const older = new Promise<{ preferences: Record<string, string> }>(r => { resolveOlder = r; });
    vi.spyOn(apiClient, 'getPreferences')
      .mockReturnValueOnce(older)
      .mockResolvedValueOnce({ preferences: { 'theme-mode': 'dark' } });

    const first = loadPreferences();
    const second = loadPreferences();
    await second;
    resolveOlder({ preferences: { 'theme-mode': 'light' } });
    await first;

    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'dark' } });
  });

  it('a stale failure does not regress an already-loaded newer result', async () => {
    let rejectOlder!: (e: unknown) => void;
    const older = new Promise<{ preferences: Record<string, string> }>((_r, rej) => { rejectOlder = rej; });
    vi.spyOn(apiClient, 'getPreferences')
      .mockReturnValueOnce(older)
      .mockResolvedValueOnce({ preferences: { 'theme-mode': 'dark' } });

    const first = loadPreferences();
    const second = loadPreferences();
    await second;
    rejectOlder(new ApiError(500, 'stale failure'));
    await first;

    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'dark' } });
  });

  it('a later call still starts a fresh fetch once the first has settled', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: {} });

    await loadPreferences();
    await loadPreferences();

    expect(apiClient.getPreferences).toHaveBeenCalledTimes(2);
  });
});

describe('loadPreferences — skip re-apply when theme unchanged (iOS matchMedia flash)', () => {
  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('does not rewrite data-theme-mode on a refetch when the theme value matches the previously applied value', async () => {
    applyThemeMode('light');

    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({
      preferences: { 'theme-mode': 'light' },
    });

    const setAttrSpy = vi.spyOn(document.documentElement, 'setAttribute');
    await loadPreferences();

    const themeWrites = setAttrSpy.mock.calls.filter((c) => c[0] === 'data-theme-mode');
    expect(themeWrites).toHaveLength(0);
  });

  it('still rewrites data-theme-mode when the value differs from the previously applied value', async () => {
    applyThemeMode('light');

    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({
      preferences: { 'theme-mode': 'dark' },
    });

    const setAttrSpy = vi.spyOn(document.documentElement, 'setAttribute');
    await loadPreferences();

    const themeWrites = setAttrSpy.mock.calls.filter((c) => c[0] === 'data-theme-mode');
    expect(themeWrites.length).toBeGreaterThan(0);
  });
});

describe("applyThemeMode('system'): following the OS", () => {
  let originalMatchMedia: typeof window.matchMedia;
  let originalSetAttribute: unknown;
  let originalGetAttribute: unknown;
  let mqListeners: Array<() => void>;
  let mqLight: boolean;
  let painted: string[];
  let attrs: Record<string, string>;

  function setVisibility(state: 'visible' | 'hidden'): void {
    (document as { visibilityState: string }).visibilityState = state;
  }

  function fireMediaQueryChange(): void {
    for (const fn of [...mqListeners]) fn();
  }

  function settle(): void {
    vi.advanceTimersByTime(SYSTEM_THEME_MODE_SETTLE_MS + SYSTEM_THEME_MODE_CONFIRM_MS);
  }

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    preferences.value = { status: 'loaded', data: { 'theme-mode': 'system' } };
    setVisibility('visible');
    mqListeners = [];
    mqLight = false;
    painted = [];
    attrs = {};
    const el = document.documentElement as unknown as Record<string, unknown>;
    originalSetAttribute = el.setAttribute;
    originalGetAttribute = el.getAttribute;
    el.setAttribute = (k: string, v: string) => {
      attrs[k] = v;
      if (k === 'data-theme-mode') painted.push(v);
    };
    el.getAttribute = (k: string) => attrs[k] ?? null;
    originalMatchMedia = window.matchMedia;
    (window as any).matchMedia = () => ({
      get matches() { return mqLight; },
      addEventListener: (_t: string, fn: () => void) => { mqListeners.push(fn); },
      removeEventListener: (_t: string, fn: () => void) => {
        const i = mqListeners.indexOf(fn);
        if (i >= 0) mqListeners.splice(i, 1);
      },
    });
  });

  afterEach(() => {
    setVisibility('visible');
    applyThemeMode('dark');
    (window as any).matchMedia = originalMatchMedia;
    const el = document.documentElement as unknown as Record<string, unknown>;
    el.setAttribute = originalSetAttribute;
    el.getAttribute = originalGetAttribute;
    vi.useRealTimers();
  });

  it('subscribes on every platform, iOS included', () => {
    applyThemeMode('system');
    expect(mqListeners).toHaveLength(1);
  });

  it('keeps a flip a repaint raced, instead of swapping the query out from under it', () => {
    const queries: Array<{ born: boolean; listeners: Array<() => void> }> = [];
    (window as any).matchMedia = () => {
      const query = { born: mqLight, listeners: [] as Array<() => void> };
      queries.push(query);
      return {
        get matches() { return mqLight; },
        addEventListener: (_t: string, fn: () => void) => { query.listeners.push(fn); },
        removeEventListener: (_t: string, fn: () => void) => {
          query.listeners = query.listeners.filter((l) => l !== fn);
        },
      };
    };
    applyThemeMode('system');
    painted = [];

    mqLight = true;
    applyThemeMode('system');
    for (const query of queries) {
      if (query.born !== mqLight) query.listeners.forEach((fn) => fn());
    }
    settle();

    expect(painted).toContain('light');
  });

  it('applies an OS flip announced while the page is visible', () => {
    applyThemeMode('system');
    painted = [];

    mqLight = true;
    fireMediaQueryChange();
    settle();

    expect(painted).toEqual(['light']);
  });

  it('catches a flip that landed before the page subscribed', async () => {
    localStorage.setItem('lucidos-theme-mode', 'system');
    attrs['data-theme-mode'] = 'dark';
    mqLight = true;
    vi.resetModules();
    await import('./preferences');
    painted = [];

    settle();

    expect(painted).toEqual(['light']);
  });

  it('ignores a flip announced while the page is hidden', () => {
    applyThemeMode('system');
    painted = [];

    setVisibility('hidden');
    mqLight = true;
    fireMediaQueryChange();
    settle();

    expect(painted).toEqual([]);
  });

  it('re-reads at settle time rather than trusting the event that woke it', () => {
    applyThemeMode('system');
    painted = [];

    mqLight = true;
    fireMediaQueryChange();
    mqLight = false;
    settle();

    expect(painted).toEqual([]);
  });

  it('repairs on resume a flip that arrived while the page was hidden', () => {
    applyThemeMode('system');
    painted = [];

    setVisibility('hidden');
    mqLight = true;
    fireMediaQueryChange();
    settle();
    expect(painted).toEqual([]);

    setVisibility('visible');
    document.dispatchEvent(new Event('visibilitychange'));
    settle();

    expect(painted).toEqual(['light']);
  });

  it('paints nothing a brief wake reads, when the page hides before the flip holds', () => {
    applyThemeMode('system');
    painted = [];

    mqLight = true;
    document.dispatchEvent(new Event('visibilitychange'));
    vi.advanceTimersByTime(SYSTEM_THEME_MODE_SETTLE_MS);
    expect(painted).toEqual([]);

    setVisibility('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    mqLight = false;
    settle();

    expect(painted).toEqual([]);
  });

  it('paints nothing when the confirming read no longer sees the flip', () => {
    applyThemeMode('system');
    painted = [];

    mqLight = true;
    document.dispatchEvent(new Event('visibilitychange'));
    vi.advanceTimersByTime(SYSTEM_THEME_MODE_SETTLE_MS);
    mqLight = false;
    settle();

    expect(painted).toEqual([]);
  });

  it('starts over on a wake after a hide cancelled the confirming read', () => {
    applyThemeMode('system');
    painted = [];

    mqLight = true;
    document.dispatchEvent(new Event('visibilitychange'));
    vi.advanceTimersByTime(SYSTEM_THEME_MODE_SETTLE_MS);
    setVisibility('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    setVisibility('visible');
    document.dispatchEvent(new Event('visibilitychange'));
    vi.advanceTimersByTime(SYSTEM_THEME_MODE_CONFIRM_MS);
    expect(painted).toEqual([]);

    vi.advanceTimersByTime(SYSTEM_THEME_MODE_SETTLE_MS);
    expect(painted).toEqual(['light']);
  });

  it('costs one apply per wake, however many resume events it delivers', () => {
    applyThemeMode('system');
    painted = [];

    mqLight = true;
    document.dispatchEvent(new Event('visibilitychange'));
    window.dispatchEvent(new Event('focus'));
    window.dispatchEvent(new Event('pageshow'));
    settle();

    expect(painted).toEqual(['light']);
  });

  it('writes nothing when the OS still says what is already painted', () => {
    applyThemeMode('system');
    painted = [];

    document.dispatchEvent(new Event('visibilitychange'));
    settle();

    expect(painted).toEqual([]);
  });

  it('leaves an explicit light or dark preference alone', () => {
    preferences.value = { status: 'loaded', data: { 'theme-mode': 'dark' } };
    applyThemeMode('dark');
    expect(mqListeners).toHaveLength(0);
    painted = [];

    mqLight = true;
    document.dispatchEvent(new Event('visibilitychange'));
    window.dispatchEvent(new Event('focus'));
    settle();

    expect(painted).toEqual([]);
  });

  it('drops a pending refresh when the user picks an explicit theme', () => {
    applyThemeMode('system');
    mqLight = true;
    fireMediaQueryChange();

    preferences.value = { status: 'loaded', data: { 'theme-mode': 'dark' } };
    applyThemeMode('dark');
    painted = [];
    settle();

    expect(painted).toEqual([]);
  });

  describe('a repaint keeps the painted mode (ADR 0092)', () => {
    it('a theme re-lay while hidden paints nothing the OS says', async () => {
      applyThemeMode('system');
      painted = [];

      setVisibility('hidden');
      mqLight = true;
      await refreshActiveTheme();

      expect(painted).not.toContain('light');
      expect(attrs['data-theme-mode']).toBe('dark');
    });

    it('a theme re-lay inside the settle window neither paints the flip early nor loses it', async () => {
      applyThemeMode('system');
      painted = [];

      mqLight = true;
      fireMediaQueryChange();
      vi.advanceTimersByTime(SYSTEM_THEME_MODE_SETTLE_MS / 2);
      await refreshActiveTheme();
      expect(painted).not.toContain('light');

      settle();
      expect(attrs['data-theme-mode']).toBe('light');
    });

    it('a theme re-lay after the follower moved the mode keeps the new mode', async () => {
      applyThemeMode('system');
      mqLight = true;
      fireMediaQueryChange();
      settle();
      painted = [];

      await refreshActiveTheme();

      expect(attrs['data-theme-mode']).toBe('light');
    });

    it('a change of preference into system resolves against the OS at once', () => {
      applyThemeMode('dark');
      painted = [];

      mqLight = true;
      applyThemeMode('system');

      expect(painted).toEqual(['light']);
    });
  });
});

describe('applyThemeMode: native title-bar tint (Tauri)', () => {
  beforeEach(() => {
    setTitlebarColorMock.mockClear();
    windowReadyToShowMock.mockClear();
    platformMocks.isTauri = false;
  });

  afterEach(() => {
    platformMocks.isTauri = false;
  });

  it('does not tint the title bar outside Tauri (web / PWA)', () => {
    applyThemeMode('light');
    expect(setTitlebarColorMock).not.toHaveBeenCalled();
  });

  it('tints the title bar the header-top blue per theme inside Tauri', () => {
    platformMocks.isTauri = true;
    applyThemeMode('light');
    expect(setTitlebarColorMock).toHaveBeenLastCalledWith(THEME_MODE_HEADER_TOP.light);
    applyThemeMode('dark');
    expect(setTitlebarColorMock).toHaveBeenLastCalledWith(THEME_MODE_HEADER_TOP.dark);
  });

  it('tells the shell the page is ready to show, Tauri only', () => {
    applyThemeMode('light');
    expect(windowReadyToShowMock).not.toHaveBeenCalled();

    platformMocks.isTauri = true;
    applyThemeMode('light');
    expect(windowReadyToShowMock).toHaveBeenCalled();
  });
});

describe('theme-color: the bar above the page continues the header', () => {
  let themeColor: string | undefined;
  let querySpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    themeColor = undefined;
    const meta = { setAttribute: (name: string, value: string) => { if (name === 'content') themeColor = value; } };
    querySpy = vi.spyOn(document, 'querySelector').mockImplementation(
      (selector: string) => (selector === 'meta[name="theme-color"]' ? meta : null) as Element | null,
    );
    resolvedTokenHexMock.mockImplementation(() => null);
  });

  afterEach(() => {
    resolvedTokenHexMock.mockImplementation(() => null);
    applyStyleOverrides({});
    querySpy.mockRestore();
  });

  const headerTopIs = (hex: string) => (name: string) => (name === '--header-bar-top' ? hex : null);

  it("takes the mode's header colour while no stylesheet defines the token", () => {
    applyThemeMode('light');
    expect(themeColor).toBe(THEME_MODE_HEADER_TOP.light);
    applyThemeMode('dark');
    expect(themeColor).toBe(THEME_MODE_HEADER_TOP.dark);
  });

  it('takes the painted header token, so a theme and the iOS nudge reach the bar', () => {
    resolvedTokenHexMock.mockImplementation(headerTopIs('#15579c'));
    applyThemeMode('dark');
    expect(themeColor).toBe('#15579c');
  });

  it('follows a style override of the header token', () => {
    applyThemeMode('dark');
    resolvedTokenHexMock.mockImplementation(headerTopIs('#336699'));
    applyStyleOverrides({ '--header-bar-top': '#336699' });
    expect(themeColor).toBe('#336699');
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
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await dismissWelcomeSuggestions();

    expect(spy).toHaveBeenCalledWith('welcome_suggestions_dismissed', 'true', undefined);
    expect((preferences.value as { data: Record<string, string> }).data.welcome_suggestions_dismissed).toBe('true');
  });

  it('dismissWelcomeSuggestions is idempotent — skips the write when already dismissed', async () => {
    preferences.value = { status: 'loaded', data: { welcome_suggestions_dismissed: 'true' } };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await dismissWelcomeSuggestions();

    expect(spy).not.toHaveBeenCalled();
  });
});

describe('retireWelcomeAfterUse: the welcome retires itself after enough threads', () => {
  const thread = (meta: Partial<ThreadState['meta']> = {}): ThreadState => ({
    meta: { initiator: 'user', state: 'active', ...meta },
  }) as ThreadState;
  const userThreads = (n: number) => Array.from({ length: n }, () => thread());

  beforeEach(() => {
    preferences.value = { status: 'loaded', data: {} };
    llmConfigured.value = true;
  });

  afterEach(() => {
    vi.restoreAllMocks();
    llmConfigured.value = true;
  });

  it('keeps the provider-setup welcome while no provider is configured', async () => {
    llmConfigured.value = false;
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await retireWelcomeAfterUse(userThreads(WELCOME_RETIRES_AFTER_THREADS));

    expect(spy).not.toHaveBeenCalled();
  });

  it(`saves the dismissal once the user has started ${WELCOME_RETIRES_AFTER_THREADS} threads`, async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await retireWelcomeAfterUse(userThreads(WELCOME_RETIRES_AFTER_THREADS));

    expect(spy).toHaveBeenCalledWith('welcome_suggestions_dismissed', 'true', undefined);
    expect(welcomeSuggestionsDismissed()).toBe(true);
  });

  it('keeps the welcome while the user has started fewer threads', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await retireWelcomeAfterUse(userThreads(WELCOME_RETIRES_AFTER_THREADS - 1));

    expect(spy).not.toHaveBeenCalled();
    expect(welcomeSuggestionsDismissed()).toBe(false);
  });

  it('counts only top-level threads the user sent, not drafts, trigger runs, sub-threads or Home', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await retireWelcomeAfterUse([
      ...userThreads(WELCOME_RETIRES_AFTER_THREADS - 1),
      thread({ state: 'composing' }),
      thread({ state: 'discarded' }),
      thread({ initiator: 'system' }),
      thread({ parentThreadId: 'parent' }),
      thread({ home: true }),
    ]);

    expect(spy).not.toHaveBeenCalled();
  });

  it('writes nothing while preferences are not loaded', async () => {
    preferences.value = { status: 'not-loaded' };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await retireWelcomeAfterUse(userThreads(WELCOME_RETIRES_AFTER_THREADS));

    expect(spy).not.toHaveBeenCalled();
  });
});

describe('backup reminder: is backup actually on?', () => {
  const T0 = Date.parse('2026-08-04T09:00:00Z');
  const DAY_MS = 24 * 60 * 60 * 1000;
  const BACKUP_ON = { backup_schedule: '0 0 3 * * *', backup_provider: 'google_drive' };

  beforeEach(() => {
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('counts backup as set up once a provider is chosen, whatever the schedule', () => {
    expect(backupIsConfigured(BACKUP_ON)).toBe(true);
    expect(backupIsConfigured({})).toBe(false);
    expect(backupIsConfigured({ backup_schedule: '0 0 3 * * *' })).toBe(false);
    expect(backupIsConfigured({ backup_provider: 'google_drive', backup_schedule: 'off' })).toBe(true);
    expect(backupIsConfigured({ backup_provider: 'google_drive' })).toBe(true);
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
    expect(backupReminderHiddenByDismissal('yesterday', T0)).toBe(false);
    expect(backupReminderNextDismissal('yesterday', T0)).toBe(new Date(T0).toISOString());
  });

  it('records the instant on the first dismissal and forever on the second', () => {
    expect(backupReminderNextDismissal(undefined, T0)).toBe(new Date(T0).toISOString());
    expect(backupReminderNextDismissal('', T0)).toBe(new Date(T0).toISOString());
    const first = new Date(T0).toISOString();
    expect(backupReminderNextDismissal(first, T0 + 31 * DAY_MS)).toBe(BACKUP_REMINDER_FOREVER);
    expect(backupReminderNextDismissal(BACKUP_REMINDER_FOREVER, T0)).toBe(BACKUP_REMINDER_FOREVER);
  });

  it('backup being on beats any dismissal state', () => {
    expect(backupReminderVisibleIn({}, T0)).toBe(true);
    expect(backupReminderVisibleIn(BACKUP_ON, T0)).toBe(false);
    expect(backupReminderVisibleIn({ ...BACKUP_ON, backup_reminder_dismissed: '' }, T0)).toBe(false);
    expect(backupReminderVisibleIn({ backup_reminder_dismissed: new Date(T0).toISOString() }, T0 + 31 * DAY_MS)).toBe(true);
  });

  it('fails closed while preferences are not loaded (no flash for returning users)', () => {
    for (const state of [
      { status: 'not-loaded' } as const,
      { status: 'loading' } as const,
      { status: 'failed', error: 'network error' } as const,
    ]) {
      preferences.value = state;
      expect(backupBanner(T0)).toBeNull();
    }
    preferences.value = { status: 'loaded', data: {} };
    expect(backupBanner(T0)).toEqual({ state: 'setup' });
  });

  describe('the overdue state', () => {
    const OVERDUE = { status: 'loaded', data: { at: null, stale: true, configured: true, overdue: true, overdue_at: null } } as const;
    const FRESH = { status: 'loaded', data: { at: null, stale: false, configured: true, overdue: false, overdue_at: null } } as const;
    const MANUAL_ONLY = { backup_provider: 'google_drive', backup_schedule: 'off' };

    it('shows when a provider is set and the engine says overdue, schedule or not', () => {
      expect(backupBannerIn(MANUAL_ONLY, OVERDUE, T0)).toEqual({ state: 'overdue', provider: 'google_drive', lastBackupAt: null });
      expect(backupBannerIn(BACKUP_ON, OVERDUE, T0)?.state).toBe('overdue');
    });

    it('carries the last backup instant for the sentence', () => {
      const at = new Date(T0 - 12 * DAY_MS).toISOString();
      const loaded = { status: 'loaded', data: { at, stale: true, configured: true, overdue: true, overdue_at: null } } as const;
      expect(backupBannerIn(BACKUP_ON, loaded, T0)).toMatchObject({ lastBackupAt: at });
    });

    it('stays hidden when the engine says the backup is recent', () => {
      expect(backupBannerIn(BACKUP_ON, FRESH, T0)).toBeNull();
    });

    it('appears once an open tab passes overdue_at, with no re-read', () => {
      const due = new Date(T0 + DAY_MS).toISOString();
      const read = { status: 'loaded', data: { ...FRESH.data, at: new Date(T0 - 6 * DAY_MS).toISOString(), overdue_at: due } } as const;
      expect(backupBannerIn(BACKUP_ON, read, T0 + DAY_MS)).toBeNull();
      expect(backupBannerIn(BACKUP_ON, read, T0 + DAY_MS + 1)?.state).toBe('overdue');
    });

    it('setup wins, and overdue never shows without a provider', () => {
      expect(backupBannerIn({}, OVERDUE, T0)).toEqual({ state: 'setup' });
      const dismissedSetup = { backup_reminder_dismissed: new Date(T0).toISOString() };
      expect(backupBannerIn(dismissedSetup, OVERDUE, T0)).toBeNull();
    });

    it('fails closed while the verdict is unloaded or failed', () => {
      for (const state of [
        { status: 'not-loaded' } as const,
        { status: 'loading' } as const,
        { status: 'failed', error: 'HTTP 500' } as const,
      ]) {
        expect(backupBannerIn(BACKUP_ON, state, T0)).toBeNull();
      }
    });

    it('a dismissal snoozes it for 7 days, then it returns', () => {
      const at = new Date(T0).toISOString();
      const dismissed = { ...BACKUP_ON, backup_overdue_reminder_dismissed: at };
      expect(backupBannerIn(dismissed, OVERDUE, T0 + 6 * DAY_MS)).toBeNull();
      expect(backupBannerIn(dismissed, OVERDUE, T0 + BACKUP_OVERDUE_SNOOZE_MS)?.state).toBe('overdue');
      expect(backupOverdueHiddenByDismissal('yesterday', T0)).toBe(false);
      expect(backupOverdueHiddenByDismissal('', T0)).toBe(false);
    });

    it('the setup dismissal does not silence overdue, and the reverse', () => {
      const setupDismissed = { ...BACKUP_ON, backup_reminder_dismissed: 'forever' };
      expect(backupBannerIn(setupDismissed, OVERDUE, T0)?.state).toBe('overdue');
    });

    it('dismissBackupOverdueReminder writes the instant to its own preference', async () => {
      preferences.value = { status: 'loaded', data: BACKUP_ON };
      const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });
      await dismissBackupOverdueReminder(T0);
      expect(spy).toHaveBeenCalledWith('backup_overdue_reminder_dismissed', new Date(T0).toISOString(), undefined);
    });

    it('reads the live signals through backupBanner', () => {
      preferences.value = { status: 'loaded', data: BACKUP_ON };
      lastSuccessfulBackup.value = OVERDUE;
      expect(backupBanner(T0)?.state).toBe('overdue');
      lastSuccessfulBackup.value = { status: 'not-loaded' };
      expect(backupBanner(T0)).toBeNull();
    });
  });

  it('dismissBackupReminder writes the instant, then forever', async () => {
    preferences.value = { status: 'loaded', data: {} };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await dismissBackupReminder(T0);
    expect(spy).toHaveBeenCalledWith('backup_reminder_dismissed', new Date(T0).toISOString(), undefined);

    await dismissBackupReminder(T0 + 31 * DAY_MS);
    expect(spy).toHaveBeenLastCalledWith('backup_reminder_dismissed', BACKUP_REMINDER_FOREVER, undefined);
  });

  it('dismissBackupReminder is a no-op while preferences are unloaded', async () => {
    preferences.value = { status: 'not-loaded' };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

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
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await setInAppBrowser(true);

    expect(spy).toHaveBeenCalledWith('experimental_in_app_browser', 'true', undefined);
    expect((preferences.value as { data: Record<string, string> }).data.experimental_in_app_browser).toBe('true');
  });
});

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
    preferences.value = { status: 'loaded', data: { external_link_target: 'chrome' } };
    expect(currentExternalLinkTarget()).toBe('safari');
  });

  it('setExternalLinkTarget persists the chosen mode', async () => {
    preferences.value = { status: 'loaded', data: {} };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await setExternalLinkTarget('ask');

    expect(spy).toHaveBeenCalledWith('external_link_target', 'ask', undefined);
    expect((preferences.value as { data: Record<string, string> }).data.external_link_target).toBe('ask');
  });
});

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
    platformMocks.isIOSPwa = true;
    preferences.value = { status: 'loading' };
    expect(externalLinkTargetConfigurable()).toBe(true);
  });
});

describe('preference writes survive an iOS PWA suspend', () => {
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
      .mockResolvedValueOnce({ success: true });

    await savePreference('theme-mode', 'light');

    expect(spy).toHaveBeenCalledTimes(2);
    expect(toasts.value).toHaveLength(0);
    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
  });

  it('speaks up when the engine refuses, rather than retrying or parking', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockRejectedValue(
      new Error("'response_styles' holds at most 20 styles (got 21)"),
    );

    await savePreference('response_styles', '[]');

    expect(spy).toHaveBeenCalledTimes(1);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('at most 20 styles');
    expect(toasts.value[0].message).not.toContain('200');
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

    spy.mockResolvedValue({ success: true });
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
    spy.mockResolvedValue({ success: true });
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

    await savePreference('theme-mode', 'light');
    await savePreference('font-family', 'inter');
    await savePreference('ui-scale', '125', undefined, true);

    expect(toasts.value).toHaveLength(1);
  });

  it('speaks once when writes keep failing to arrive, naming what is stuck', async () => {
    vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());

    await savePreference('theme-mode', 'light');
    expect(toasts.value).toHaveLength(0);
    await savePreference('font-family', 'inter');
    expect(toasts.value).toHaveLength(0);

    await savePreference('ui-scale', '125', undefined, true);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('font-family, theme-mode, ui-scale');
  });

  it('retracts the unreachable banner once the queue drains', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());
    await savePreference('theme-mode', 'light');
    await savePreference('font-family', 'inter');
    await savePreference('ui-scale', '125', undefined, true);
    expect(toasts.value).toHaveLength(1);

    spy.mockResolvedValue({ success: true });
    await flushPendingPreferenceWrites();

    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
    expect(toasts.value).toHaveLength(0);
  });

  it('applies the value locally before the network call, so a failed write still shows', async () => {
    vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());
    const applied = vi.fn();

    await savePreference('theme-mode', 'light', applied);

    expect(applied).toHaveBeenCalledTimes(1);
    expect(preferences.value).toMatchObject({ data: { 'theme-mode': 'light' } });
  });
});

describe('a refetch during a write keeps what the user chose', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    _resetPendingPreferenceWritesForTesting();
    toasts.value = [];
    preferences.value = { status: 'loaded', data: { 'theme-mode': 'dark' } };
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: { 'theme-mode': 'dark' } });
  });

  afterEach(() => {
    _resetPendingPreferenceWritesForTesting();
    toasts.value = [];
  });

  function heldWrite(): { answer: (outcome: 'accept' | 'refuse') => void } {
    let answer: (outcome: 'accept' | 'refuse') => void = () => {};
    vi.spyOn(apiClient, 'setPreference').mockImplementation(() => new Promise((resolve, reject) => {
      answer = (outcome) => (outcome === 'accept' ? resolve({ success: true }) : reject(new Error('refused')));
    }));
    return { answer: (outcome) => answer(outcome) };
  }

  it('until the engine accepts it', async () => {
    const write = heldWrite();
    const saved = savePreference('theme-mode', 'light');

    await loadPreferences();
    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'light' } });

    write.answer('accept');
    await saved;
    await loadPreferences();
    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'dark' } });
  });

  it('when the refetch started before the engine accepted it', async () => {
    const write = heldWrite();
    const saved = savePreference('theme-mode', 'light');
    let answerRead: () => void = () => {};
    vi.spyOn(apiClient, 'getPreferences').mockImplementationOnce(() => new Promise((resolve) => {
      answerRead = () => resolve({ preferences: { 'theme-mode': 'dark' } });
    }));
    const loaded = loadPreferences();
    await vi.waitFor(() => expect(apiClient.setPreference).toHaveBeenCalled());

    write.answer('accept');
    await saved;
    answerRead();
    await loaded;
    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'light' } });
  });

  it('but not one the engine refused while the refetch was out', async () => {
    const write = heldWrite();
    const saved = savePreference('theme-mode', 'light');
    let answerRead: () => void = () => {};
    vi.spyOn(apiClient, 'getPreferences').mockImplementationOnce(() => new Promise((resolve) => {
      answerRead = () => resolve({ preferences: { 'theme-mode': 'dark' } });
    }));
    const loaded = loadPreferences();
    await vi.waitFor(() => expect(apiClient.setPreference).toHaveBeenCalled());

    write.answer('refuse');
    await saved;
    answerRead();
    await loaded;
    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'dark' } });
  });

  it('until the engine refuses it', async () => {
    const write = heldWrite();
    const saved = savePreference('theme-mode', 'light');
    await vi.waitFor(() => expect(apiClient.setPreference).toHaveBeenCalled());
    write.answer('refuse');
    await saved;

    await loadPreferences();
    expect(preferences.value).toEqual({ status: 'loaded', data: { 'theme-mode': 'dark' } });
  });
});

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

    await Promise.all([
      savePreference('ui-scale', '112.5', undefined, true),
      savePreference('ui-scale', '150', undefined, true),
    ]);

    expect(sent).toEqual(['150']);
    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
  });

  it('a parked write queued behind a newer one that landed stands down', async () => {
    const sent: string[] = [];
    let acceptNewer: () => void = () => {};
    vi.spyOn(apiClient, 'setPreference').mockImplementation((_k, v) => {
      sent.push(v);
      if (v === '112.5') return Promise.reject(cancelled());
      return new Promise<ApiResult>((resolve) => { acceptNewer = () => resolve({ success: true }); });
    });
    await savePreference('ui-scale', '112.5', undefined, true);
    expect(_pendingPreferenceKeysForTesting()).toEqual(['ui-scale']);

    const newer = savePreference('ui-scale', '150', undefined, true);
    await vi.waitFor(() => expect(sent).toContain('150'));
    const flushed = flushPendingPreferenceWrites();
    acceptNewer();
    await Promise.all([newer, flushed]);

    expect(sent).toEqual(['112.5', '112.5', '150']);
    expect(_pendingPreferenceKeysForTesting()).toEqual([]);
  });

  it('does not re-send a superseded write between its own two attempts', async () => {
    const sent: string[] = [];
    let failFirstAttempt: (() => void) | null = null;
    let markDispatched: () => void = () => {};
    const firstDispatched = new Promise<void>(r => { markDispatched = r; });

    vi.spyOn(apiClient, 'setPreference').mockImplementation((_k, v) => {
      sent.push(v);
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

    expect(sent.filter(v => v === '112.5')).toHaveLength(1);
    expect(sent.filter(v => v === '150')).toHaveLength(2);
    expect(_pendingPreferenceKeysForTesting()).toEqual(['ui-scale']);
  });

  it('keeps a newer parked write when an older one is rejected outright', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference');
    spy.mockRejectedValue(cancelled());
    await savePreference('ui-scale', '125', undefined, true);
    expect(_pendingPreferenceKeysForTesting()).toEqual(['ui-scale']);

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
      savePreference('theme-mode', 'light'),
      savePreference('font-family', 'inter'),
      savePreference('ui-scale', '125', undefined, true),
    ]);

    expect(maxInFlight).toBeGreaterThan(1);
  });
});

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

  async function stallThreeWrites() {
    const spy = vi.spyOn(apiClient, 'setPreference').mockRejectedValue(cancelled());
    await savePreference('theme-mode', 'light');
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
    expect(toasts.value.some(t => t.message.includes('unknown preference key'))).toBe(true);
  });

  it('treats a rejection as proof the engine is reachable, resetting the count', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference');
    spy.mockRejectedValue(cancelled());
    await savePreference('theme-mode', 'light');
    await savePreference('font-family', 'inter');

    spy.mockRejectedValue(new ApiError(400, 'nope'));
    await savePreference('image_model', 'auto');
    toasts.value = toasts.value.filter(t => t.key !== 'preference-save-rejected');

    spy.mockRejectedValue(cancelled());
    await savePreference('timezone', 'UTC');
    expect(toasts.value.some(t => t.key === UNREACHABLE)).toBe(false);
  });

  it('a successful save does not churn the toast signal when nothing is showing', async () => {
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });
    const before = toasts.value;

    await savePreference('theme-mode', 'light');

    expect(toasts.value).toBe(before);
  });
});


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
    preferences.value = { status: 'loaded', data: { max_tool_calls: '1000000' } };
    expect(currentMaxToolCalls()).toBe(1_000_000);
  });

  it('tolerates surrounding whitespace, as the engine does', () => {
    preferences.value = { status: 'loaded', data: { max_tool_calls: ' 750 ' } };
    expect(currentMaxToolCalls()).toBe(750);
  });

  it('reads a value outside the catalog bounds as unset, as the engine does', () => {
    for (const raw of ['0', '-5', String(MAX_TOOL_CALLS_MAX + 1), '1'.repeat(400)]) {
      preferences.value = { status: 'loaded', data: { max_tool_calls: raw } };
      expect(currentMaxToolCalls(), raw).toBe(MAX_TOOL_CALLS_DEFAULT);
    }
    preferences.value = { status: 'loaded', data: { max_tool_calls: String(MAX_TOOL_CALLS_MAX) } };
    expect(currentMaxToolCalls()).toBe(MAX_TOOL_CALLS_MAX);
  });

  it('reads a number the way the engine casts it, and anything else as unset', () => {
    for (const [raw, expected] of [['12.5', 12], ['1e3', 1000]] as const) {
      preferences.value = { status: 'loaded', data: { max_tool_calls: raw } };
      expect(currentMaxToolCalls(), raw).toBe(expected);
    }
    for (const raw of ['', '   ', 'abc', '1_000']) {
      preferences.value = { status: 'loaded', data: { max_tool_calls: raw } };
      expect(currentMaxToolCalls(), `stored ${JSON.stringify(raw)}`).toBe(MAX_TOOL_CALLS_DEFAULT);
    }
  });
});

describe('a stored background selection', () => {
  it('reads null for both halves while unset, whatever the keys it inherits hold', () => {
    preferences.value = {
      status: 'loaded',
      data: { model_title: 'gemini-3.8-flash', reasoning_title: 'low' },
    };
    expect(storedBackgroundSelection('model_change_summary', 'reasoning_change_summary'))
      .toEqual({ model: null, effort: null });
  });

  it('reads what is stored', () => {
    preferences.value = {
      status: 'loaded',
      data: { model_summary_compaction: 'claude-fable-5-1', reasoning_summary_compaction: 'medium' },
    };
    expect(storedBackgroundSelection('model_summary_compaction', 'reasoning_summary_compaction'))
      .toEqual({ model: 'claude-fable-5-1', effort: 'medium' });
  });

  it('ignores a stored tier the catalog does not accept', () => {
    preferences.value = { status: 'loaded', data: { reasoning_summary_compaction: 'turbo' } };
    expect(storedBackgroundSelection('model_summary_compaction', 'reasoning_summary_compaction').effort)
      .toBeNull();
  });
});

describe('the memory module', () => {
  afterEach(() => vi.restoreAllMocks());

  it('runs Classic, with no choice, while unset, unloaded or unknown', () => {
    const states: Loadable<Record<string, string>>[] = [
      { status: 'not-loaded' },
      { status: 'loaded', data: {} },
      { status: 'loaded', data: { memory_module: 'bogus' } },
    ];
    for (const value of states) {
      preferences.value = value;
      expect(storedMemoryModule()).toBeNull();
      expect(currentMemoryModule()).toBe(MEMORY_MODULE_WHILE_UNSET);
    }
  });

  it('reads Classic once set', () => {
    preferences.value = { status: 'loaded', data: { memory_module: 'classic' } };
    expect(storedMemoryModule()).toBe('classic');
    expect(currentMemoryModule()).toBe('classic');
  });

  it('reads Tree once set', () => {
    preferences.value = { status: 'loaded', data: { memory_module: 'tree' } };
    expect(storedMemoryModule()).toBe('tree');
    expect(currentMemoryModule()).toBe('tree');
  });

  it('writes the global key and applies it locally', async () => {
    preferences.value = { status: 'loaded', data: {} };
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true });

    await setMemoryModule('tree');

    expect(spy).toHaveBeenCalledWith('memory_module', 'tree', undefined);
    expect(currentMemoryModule()).toBe('tree');
  });
});

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

describe('notification_toasts: the mirror that survives a cold start', () => {
  const KEY = 'lucidos-notification-toasts';

  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('answers from the mirror while preferences are still loading', () => {
    localStorage.setItem(KEY, 'false');
    preferences.value = { status: 'loading' };
    expect(currentNotificationToasts()).toBe(false);
  });

  it('answers from the mirror when the load failed', () => {
    localStorage.setItem(KEY, 'false');
    preferences.value = { status: 'failed', error: 'unreachable' };
    expect(currentNotificationToasts()).toBe(false);
  });

  it('lets the served value beat the mirror', () => {
    localStorage.setItem(KEY, 'false');
    preferences.value = { status: 'loaded', data: { notification_toasts: 'true' } };
    expect(currentNotificationToasts()).toBe(true);
  });

  it('is on with neither a preference nor a mirror', () => {
    expect(currentNotificationToasts()).toBe(true);
  });

  it('takes the mirror from what the engine served', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({
      preferences: { notification_toasts: 'false' },
    });
    await loadPreferences();
    expect(localStorage.getItem(KEY)).toBe('false');
  });

  it('clears the mirror when the engine serves no value', async () => {
    localStorage.setItem(KEY, 'false');
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: {} });
    await loadPreferences();
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  it('writes the mirror on the user pick, before the round trip', () => {
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true } as ApiResult);
    void setNotificationToasts(false);
    expect(localStorage.getItem(KEY)).toBe('false');
  });
});

describe('composer_enter_key: Enter-to-send or a new line', () => {
  const KEY = 'lucidos-composer-enter-key';

  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('defaults to the catalog value with neither a preference nor a mirror', () => {
    expect(currentComposerEnterKey()).toBe(PREF_COMPOSER_ENTER_KEY.fallback);
  });

  it('answers from the mirror while preferences are still loading', () => {
    localStorage.setItem(KEY, 'newline');
    preferences.value = { status: 'loading' };
    expect(currentComposerEnterKey()).toBe('newline');
  });

  it('answers from the mirror when the load failed', () => {
    localStorage.setItem(KEY, 'newline');
    preferences.value = { status: 'failed', error: 'unreachable' };
    expect(currentComposerEnterKey()).toBe('newline');
  });

  it('lets the served value beat the mirror', () => {
    localStorage.setItem(KEY, 'newline');
    preferences.value = { status: 'loaded', data: { composer_enter_key: 'send' } };
    expect(currentComposerEnterKey()).toBe('send');
  });

  it('ignores a stored value the catalog does not accept', () => {
    preferences.value = { status: 'loaded', data: { composer_enter_key: 'shift-enter' } };
    expect(currentComposerEnterKey()).toBe(PREF_COMPOSER_ENTER_KEY.fallback);
  });

  it('takes the mirror from what the engine served', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({
      preferences: { composer_enter_key: 'newline' },
    });
    await loadPreferences();
    expect(localStorage.getItem(KEY)).toBe('newline');
  });

  it('clears the mirror when the engine serves no value', async () => {
    localStorage.setItem(KEY, 'newline');
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: {} });
    await loadPreferences();
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  it('writes the mirror on the user pick, before the round trip', () => {
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true } as ApiResult);
    void setComposerEnterKey('newline');
    expect(localStorage.getItem(KEY)).toBe('newline');
  });
});

describe('autocorrect: the switch that keeps Send reachable on iOS', () => {
  const KEY = 'lucidos-autocorrect';

  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
    platformMocks.isIOS = false;
    setProseAutocorrectMock.mockClear();
    _resetPendingPreferenceWritesForTesting();
  });

  afterEach(() => {
    platformMocks.isIOS = false;
    vi.restoreAllMocks();
  });

  it('is on when nothing is stored, on an iPhone and on a desktop alike', () => {
    preferences.value = { status: 'loaded', data: {} };
    platformMocks.isIOS = true;
    expect(currentAutocorrect()).toBe(true);
    platformMocks.isIOS = false;
    expect(currentAutocorrect()).toBe(true);
  });

  it('lets a stored off win on any client', () => {
    preferences.value = { status: 'loaded', data: { autocorrect: 'false' } };
    platformMocks.isIOS = true;
    expect(currentAutocorrect()).toBe(false);
    platformMocks.isIOS = false;
    expect(currentAutocorrect()).toBe(false);
  });

  it('answers from the mirror while preferences are still loading', () => {
    platformMocks.isIOS = true;
    localStorage.setItem(KEY, 'false');
    preferences.value = { status: 'loading' };
    expect(currentAutocorrect()).toBe(false);
  });

  it('applies the served value to the stamp and to the mirror', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({
      preferences: { autocorrect: 'false' },
    });
    await loadPreferences();
    expect(setProseAutocorrectMock).toHaveBeenLastCalledWith(false);
    expect(localStorage.getItem(KEY)).toBe('false');
  });

  it('keeps autocorrect on for a fresh iPhone that stores nothing', async () => {
    platformMocks.isIOS = true;
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: {} });
    await loadPreferences();
    expect(setProseAutocorrectMock).toHaveBeenLastCalledWith(true);
  });

  it('clears the mirror when the engine serves no value', async () => {
    localStorage.setItem(KEY, 'false');
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: {} });
    await loadPreferences();
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  it('writes this device only, and re-stamps before the round trip', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true } as ApiResult);
    const saved = setAutocorrect(false);
    expect(setProseAutocorrectMock).toHaveBeenLastCalledWith(false);
    expect(localStorage.getItem(KEY)).toBe('false');
    await saved;
    expect(spy).toHaveBeenCalledWith('autocorrect', 'false', expect.any(String));
    await setAutocorrect(true);
    expect(setProseAutocorrectMock).toHaveBeenLastCalledWith(true);
  });
});
describe('motion: the device-scoped calm switch', () => {
  const KEY = 'lucidos-motion';

  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
    motionPreference.value = 'system';
    _resetPendingPreferenceWritesForTesting();
  });

  afterEach(() => {
    motionPreference.value = 'system';
    vi.restoreAllMocks();
  });

  it('follows the OS when nothing is stored', () => {
    preferences.value = { status: 'loaded', data: {} };
    expect(currentMotion()).toBe('system');
  });

  it('answers from the mirror while preferences are still loading', () => {
    localStorage.setItem(KEY, 'reduce');
    preferences.value = { status: 'loading' };
    expect(currentMotion()).toBe('reduce');
  });

  it('applies the served value to the signal and to the mirror', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: { motion: 'full' } });
    await loadPreferences();
    expect(motionPreference.value).toBe('full');
    expect(localStorage.getItem(KEY)).toBe('full');
  });

  it('clears the mirror and follows the OS when the engine serves no value', async () => {
    localStorage.setItem(KEY, 'reduce');
    motionPreference.value = 'reduce';
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: {} });
    await loadPreferences();
    expect(localStorage.getItem(KEY)).toBeNull();
    expect(motionPreference.value).toBe('system');
  });

  it('writes this device only, and applies before the round trip', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true } as ApiResult);
    const saved = setMotion('reduce');
    expect(motionPreference.value).toBe('reduce');
    expect(localStorage.getItem(KEY)).toBe('reduce');
    await saved;
    expect(spy).toHaveBeenCalledWith('motion', 'reduce', expect.any(String));
  });
});

describe('mobile dynamic bars: always on in the installed iOS app', () => {
  beforeEach(() => {
    preferences.value = { status: 'loaded', data: {} };
    platformMocks.isIOSPwa = false;
  });

  afterEach(() => {
    platformMocks.isIOSPwa = false;
    vi.restoreAllMocks();
  });

  it('is on while nothing is stored, as the catalog says', () => {
    expect(currentMobileDynamicBars()).toBe(true);
  });

  it('follows the stored preference outside the installed iOS app', () => {
    preferences.value = { status: 'loaded', data: { mobile_dynamic_bars: 'true' } };
    expect(currentMobileDynamicBars()).toBe(true);
    preferences.value = { status: 'loaded', data: { mobile_dynamic_bars: 'false' } };
    expect(currentMobileDynamicBars()).toBe(false);
  });

  it('is on in the installed iOS app, whatever is stored', () => {
    platformMocks.isIOSPwa = true;
    preferences.value = { status: 'loaded', data: { mobile_dynamic_bars: 'false' } };
    expect(currentMobileDynamicBars()).toBe(true);
    preferences.value = { status: 'not-loaded' };
    expect(currentMobileDynamicBars()).toBe(true);
  });

  it('still saves the preference for other devices', async () => {
    const spy = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true } as ApiResult);
    await setMobileDynamicBars(false);
    expect(spy).toHaveBeenCalledWith('mobile_dynamic_bars', 'false', undefined);
  });
});

describe('the resident-block sections a call opens with', () => {
  const KEY = 'voice_resident_sections';

  function stored(): string | undefined {
    if (preferences.value.status !== 'loaded') return undefined;
    return preferences.value.data[KEY];
  }

  beforeEach(() => {
    _resetPendingPreferenceWritesForTesting();
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true } as ApiResult);
    preferences.value = { status: 'loaded', data: {} };
  });

  afterEach(() => {
    preferences.value = { status: 'not-loaded' };
    vi.restoreAllMocks();
  });

  it('reads unset as the sections that ship on', () => {
    for (const section of VOICE_RESIDENT_SECTIONS) {
      expect(voiceSectionEnabled(section.id)).toBe(DEFAULT_VOICE_RESIDENT_SECTIONS.includes(section.id));
    }
  });

  it('reads a stored list as exactly that list', () => {
    preferences.value = { status: 'loaded', data: { [KEY]: 'this-thread' } };
    expect(voiceSectionEnabled('this-thread')).toBe(true);
    expect(voiceSectionEnabled('who-and-where')).toBe(false);
  });

  it('turning one off writes the rest', async () => {
    await setVoiceSectionEnabled('this-thread', false);
    expect(stored()).toBe(DEFAULT_VOICE_RESIDENT_SECTIONS.filter((id) => id !== 'this-thread').join(','));
  });

  it('turning the last one off stores an empty list', async () => {
    preferences.value = { status: 'loaded', data: { [KEY]: 'this-thread' } };
    await setVoiceSectionEnabled('this-thread', false);
    expect(stored()).toBe('');
    expect(voiceSectionEnabled('this-thread')).toBe(false);
  });

  it('writes the registry order however they were toggled', async () => {
    preferences.value = { status: 'loaded', data: { [KEY]: 'workspace-shape' } };
    await setVoiceSectionEnabled('who-and-where', true);
    expect(stored()).toBe('who-and-where,workspace-shape');
  });

  it('keeps an id the registry does not carry', async () => {
    preferences.value = { status: 'loaded', data: { [KEY]: 'this-thread,from-the-future' } };
    await setVoiceSectionEnabled('who-and-where', true);
    expect(stored()).toBe('who-and-where,this-thread,from-the-future');
  });
});


describe('agent brand colours', () => {
  it('keeps a saved Off choice during reload and a failed preference fetch', async () => {
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true } as ApiResult);
    await setAgentBrandColours(false);
    preferences.value = { status: 'loading' };
    expect(currentAgentBrandColours()).toBe(false);
    vi.spyOn(apiClient, 'getPreferences').mockRejectedValue(new Error('offline'));
    await loadPreferences();
    expect(currentAgentBrandColours()).toBe(false);
  });

  it('repaints shared marks when preferences refresh', () => {
    const stamp = vi.spyOn(document.documentElement, 'setAttribute');
    preferences.value = { status: 'loaded', data: { 'agent-brand-colours': 'false' } };
    expect(stamp).toHaveBeenLastCalledWith('data-agent-brand-colours', 'off');
    preferences.value = { status: 'loaded', data: {} };
    expect(stamp).toHaveBeenLastCalledWith('data-agent-brand-colours', 'on');
  });

  it('loads a saved choice and restores the default after reset', async () => {
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: { 'agent-brand-colours': 'false' } });
    await loadPreferences();
    expect(currentAgentBrandColours()).toBe(false);
    vi.mocked(apiClient.getPreferences).mockResolvedValue({ preferences: {} });
    await loadPreferences();
    expect(currentAgentBrandColours()).toBe(true);
  });

  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'loaded', data: {} };
    _resetPendingPreferenceWritesForTesting();
  });
  afterEach(() => vi.restoreAllMocks());

  it('defaults to normal colours', () => {
    expect(currentAgentBrandColours()).toBe(true);
  });

  it('persists a device choice through the preference pipeline', async () => {
    const save = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true } as ApiResult);
    await setAgentBrandColours(false);
    expect(currentAgentBrandColours()).toBe(false);
    expect(save).toHaveBeenCalledWith('agent-brand-colours', 'false', expect.any(String));
  });
});
