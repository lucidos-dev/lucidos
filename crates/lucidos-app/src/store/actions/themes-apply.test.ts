/**
 * The shell's theme layer: which inline tokens land on <html>, and in what
 * order against the theme and the style overrides.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { preferences, toasts } from '../store';
import {
  applyStyleOverrides, applyThemeMode, isActiveThemePath, loadPreferences, refreshActiveTheme, setTheme,
  _resetPendingPreferenceWritesForTesting,
} from './preferences';
import * as apiClient from '../../api/client';
import { ApiError, type Theme } from '../../api/client';

vi.mock('../../utils/platform', () => ({
  isTauri: () => false,
  isIOSPwa: () => false,
  isIOS: () => false,
}));
vi.mock('../../utils/noAutofill', () => ({ setProseAutocorrect: vi.fn() }));
vi.mock('../../utils/tauri', () => ({
  setTitlebarColor: vi.fn(() => Promise.resolve()),
  windowReadyToShow: vi.fn(),
}));

const props = new Map<string, string>();
let savedStyle: unknown;

function theme(id: string, dark: Record<string, string>, light: Record<string, string> = {}): Theme {
  return { id, source: 'built-in', name: id, modes: ['dark', 'light'], resolved: { dark, light, fonts: {}, workspace_fonts: [] } };
}

beforeEach(() => {
  props.clear();
  localStorage.clear();
  const root = document.documentElement as unknown as { style: unknown };
  savedStyle = root.style;
  root.style = {
    setProperty: (name: string, value: string) => { props.set(name, value); },
    removeProperty: (name: string) => { props.delete(name); },
    getPropertyValue: (name: string) => props.get(name) ?? '',
  };
  preferences.value = { status: 'loaded', data: { 'theme-mode': 'dark' } };
  applyStyleOverrides({});
  applyThemeMode('dark');
});

afterEach(() => {
  (document.documentElement as unknown as { style: unknown }).style = savedStyle;
  vi.restoreAllMocks();
  _resetPendingPreferenceWritesForTesting();
});

describe('the active theme', () => {
  it('lays the map for the painted mode inline and caches both maps', async () => {
    vi.spyOn(apiClient, 'getTheme').mockResolvedValue(
      theme('nord', { '--accent': '#88c0d0', '--bg-primary': '#2e3440' }, { '--accent': '#5e81ac' }),
    );
    await refreshActiveTheme('nord');

    expect(props.get('--accent')).toBe('#88c0d0');
    expect(props.get('--bg-primary')).toBe('#2e3440');
    expect(JSON.parse(localStorage.getItem('lucidos-theme-resolved')!).light['--accent']).toBe('#5e81ac');
  });

  it('switches maps when the theme mode flips', async () => {
    vi.spyOn(apiClient, 'getTheme').mockResolvedValue(
      theme('nord', { '--accent': '#88c0d0', '--only-dark': '1px' }, { '--accent': '#5e81ac' }),
    );
    await refreshActiveTheme('nord');
    applyThemeMode('light');

    expect(props.get('--accent')).toBe('#5e81ac');
    expect(props.has('--only-dark')).toBe(false);
    expect(props.get('--bg-primary')).toBe('#ffffff');
  });

  it('leaves nothing behind when the next theme sets fewer tokens', async () => {
    const getTheme = vi.spyOn(apiClient, 'getTheme');
    getTheme.mockResolvedValueOnce(theme('a', { '--accent': '#111111', '--header-fg': '#eeeeee' }));
    await refreshActiveTheme('a');
    getTheme.mockResolvedValueOnce(theme('b', { '--accent': '#222222' }));
    await refreshActiveTheme('b');

    expect(props.get('--accent')).toBe('#222222');
    expect(props.has('--header-fg')).toBe(false);
  });

  it('the default theme clears the layer without a fetch', async () => {
    const getTheme = vi.spyOn(apiClient, 'getTheme')
      .mockResolvedValue(theme('a', { '--accent': '#111111' }));
    await refreshActiveTheme('a');
    await refreshActiveTheme('lucidos');

    expect(getTheme).toHaveBeenCalledTimes(1);
    expect(props.has('--accent')).toBe(false);
    expect(props.get('--bg-primary')).toBe('#07172e');
  });

  it('a theme that no longer exists paints the default', async () => {
    const getTheme = vi.spyOn(apiClient, 'getTheme');
    getTheme.mockResolvedValueOnce(theme('a', { '--accent': '#111111' }));
    await refreshActiveTheme('a');
    getTheme.mockRejectedValueOnce(new ApiError(404, "theme 'gone' not found"));
    await refreshActiveTheme('gone');

    expect(props.has('--accent')).toBe(false);
  });

  it('a theme the engine now refuses stops painting, and says why once', async () => {
    const getTheme = vi.spyOn(apiClient, 'getTheme');
    getTheme.mockResolvedValueOnce(theme('old', { '--accent': '#111111' }));
    await refreshActiveTheme('old');
    toasts.value = [];
    const refusal = new ApiError(422, '`tokens`: --z-modal is not a theme token.');
    getTheme.mockRejectedValueOnce(refusal).mockRejectedValueOnce(refusal);
    await refreshActiveTheme('old');
    await refreshActiveTheme('old');

    expect(props.has('--accent')).toBe(false);
    const warnings = toasts.value.filter(t => t.message.includes('no longer passes validation'));
    expect(warnings).toHaveLength(1);
    expect(warnings[0].message).toContain('--z-modal');
  });

  it('a refused theme the user picks again says why again', async () => {
    const getTheme = vi.spyOn(apiClient, 'getTheme');
    toasts.value = [];
    const refusal = new ApiError(422, '`tokens`: --z-modal is not a theme token.');
    getTheme.mockRejectedValueOnce(refusal).mockRejectedValueOnce(refusal);
    await refreshActiveTheme('old', true);
    await refreshActiveTheme('old', true);

    const warnings = toasts.value.filter(t => t.message.includes('no longer passes validation'));
    expect(warnings).toHaveLength(1);
    expect(warnings[0].count).toBe(2);
  });

  it('never lays a reserved name or an out-of-reach shadow inline, even handed a map directly', () => {
    applyStyleOverrides({
      '--accent': '#ff0000',
      '--protected-text': '#000000',
      '--user-ui-scale': '5%',
      '--shadow-md': '0 0 0 100vmax #000',
      '--shadow-sm': '0 2px 6px rgba(0, 0, 0, 0.2)',
    });

    expect(props.get('--accent')).toBe('#ff0000');
    expect(props.get('--shadow-sm')).toBe('0 2px 6px rgba(0, 0, 0, 0.2)');
    for (const name of ['--protected-text', '--user-ui-scale', '--shadow-md']) {
      expect(props.has(name), name).toBe(false);
    }
  });

  it('a transient failure keeps what is painted', async () => {
    const getTheme = vi.spyOn(apiClient, 'getTheme');
    getTheme.mockResolvedValueOnce(theme('a', { '--accent': '#111111' }));
    await refreshActiveTheme('a');
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    getTheme.mockRejectedValueOnce(new TypeError('Load failed'));
    await refreshActiveTheme('a');

    expect(props.get('--accent')).toBe('#111111');
  });

  it('sits under the style overrides, which keep winning after a theme change', async () => {
    preferences.value = {
      status: 'loaded',
      data: { 'theme-mode': 'dark', style_overrides: JSON.stringify({ '--accent': '#ff0000' }) },
    };
    applyStyleOverrides({ '--accent': '#ff0000' });
    vi.spyOn(apiClient, 'getTheme').mockResolvedValue(theme('a', { '--accent': '#111111' }));
    await refreshActiveTheme('a');

    expect(props.get('--accent')).toBe('#ff0000');
  });

  it('clearing an override uncovers the theme instead of the stylesheet', async () => {
    vi.spyOn(apiClient, 'getTheme').mockResolvedValue(theme('a', { '--accent': '#111111' }));
    await refreshActiveTheme('a');
    applyStyleOverrides({ '--accent': '#ff0000' });
    applyStyleOverrides({});

    expect(props.get('--accent')).toBe('#111111');
  });

  it('a failed fetch is asked for again on the next preference load', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    const getTheme = vi.spyOn(apiClient, 'getTheme').mockRejectedValueOnce(new TypeError('Load failed'));
    await refreshActiveTheme('b');
    getTheme.mockResolvedValueOnce(theme('b', { '--accent': '#444444' }));
    vi.spyOn(apiClient, 'getPreferences').mockResolvedValue({ preferences: { 'theme-mode': 'dark', theme: 'b' } });
    await loadPreferences();
    await vi.waitFor(() => expect(props.get('--accent')).toBe('#444444'));
  });

  it('setTheme fetches the theme it was given, not the one the signal still names', async () => {
    preferences.value = { status: 'loaded', data: { 'theme-mode': 'dark', theme: 'old' } };
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);
    const getTheme = vi.spyOn(apiClient, 'getTheme').mockResolvedValue(theme('new', { '--accent': '#333333' }));
    await setTheme('new');

    expect(getTheme).toHaveBeenCalledWith('new');
  });

  it('setTheme paints a map it is handed at once, then lets the fetch confirm it', async () => {
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);
    let answer: (theme: Theme) => void = () => {};
    vi.spyOn(apiClient, 'getTheme').mockReturnValue(new Promise(resolve => { answer = resolve; }));
    void setTheme('new', theme('new', { '--accent': '#555555' }).resolved);
    expect(props.get('--accent')).toBe('#555555');

    answer(theme('new', { '--accent': '#666666' }));
    await vi.waitFor(() => expect(props.get('--accent')).toBe('#666666'));
  });

  it('a fetch that confirms the painted map repaints nothing, so no transition is cut off', async () => {
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);
    const picked = theme('new', { '--accent': '#555555' });
    vi.spyOn(apiClient, 'getTheme').mockResolvedValue(picked);
    const swaps = vi.fn(() => 0);
    vi.stubGlobal('requestAnimationFrame', swaps);
    try {
      await setTheme('new', picked.resolved);
      await vi.waitFor(() => expect(apiClient.getTheme).toHaveBeenCalled());
      await Promise.resolve();
      expect(swaps).toHaveBeenCalledTimes(1);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('a pick that already shows its theme says nothing when the confirming fetch fails', async () => {
    vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.spyOn(apiClient, 'getTheme').mockRejectedValue(new TypeError('Load failed'));
    toasts.value = [];
    await refreshActiveTheme('new', true, theme('new', { '--accent': '#555555' }).resolved);

    expect(props.get('--accent')).toBe('#555555');
    expect(toasts.value).toEqual([]);
  });

  it('turns transitions off for the frame a swap paints, and back on after it', () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => frames.push(callback));
    const attributes = new Set<string>();
    const root = document.documentElement;
    vi.spyOn(root, 'setAttribute').mockImplementation(name => { attributes.add(name); });
    vi.spyOn(root, 'removeAttribute').mockImplementation(name => { attributes.delete(name); });
    try {
      const swapping = () => attributes.has('data-theme-swap');
      applyThemeMode('light');
      expect(swapping()).toBe(true);
      // The browser paints the swap after the first frame's callbacks.
      frames.shift()!(0);
      expect(swapping()).toBe(true);
      frames.shift()!(0);
      expect(swapping()).toBe(false);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('knows its own file', () => {
    preferences.value = { status: 'loaded', data: { theme: 'mine' } };
    expect(isActiveThemePath('themes/mine.json')).toBe(true);
    expect(isActiveThemePath('themes/other.json')).toBe(false);
    expect(isActiveThemePath(undefined)).toBe(false);
  });
});
