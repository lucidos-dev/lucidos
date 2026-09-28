/**
 * Picking a theme from the picker: a single-mode theme picked from its other
 * mode asks before it switches the theme to the theme's mode.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { confirmState, preferences } from '../store';
import { applyThemeMode, paintedThemeMode, _resetPendingPreferenceWritesForTesting } from './preferences';
import { pickTheme } from './themes';
import * as apiClient from '../../api/client';
import type { Theme } from '../../api/client';

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

function theme(id: string, modes: Theme['modes']): Theme {
  return { id, source: 'built-in', name: id, modes, resolved: { dark: {}, light: {}, fonts: {}, workspace_fonts: [] } };
}

let setPreference: { mock: { calls: Array<[string, string, ...unknown[]]> } };

function saved(): Record<string, string> {
  return Object.fromEntries(setPreference.mock.calls.map(([key, value]) => [key, value]));
}

/** Answer the confirm `pickTheme` raised, then let the pick finish. */
async function answer(pick: Promise<void>, ok: boolean): Promise<void> {
  await vi.waitFor(() => expect(confirmState.value.visible).toBe(true));
  confirmState.value.resolve?.(ok);
  confirmState.value = { ...confirmState.value, visible: false };
  await pick;
}

beforeEach(() => {
  localStorage.clear();
  preferences.value = { status: 'loaded', data: { 'theme-mode': 'dark', theme: 'lucidos' } };
  applyThemeMode('dark');
  setPreference = vi.spyOn(apiClient, 'setPreference').mockResolvedValue(undefined as never);
  vi.spyOn(apiClient, 'getTheme').mockImplementation(id => Promise.resolve(theme(id, ['light'])));
});

afterEach(() => {
  vi.restoreAllMocks();
  _resetPendingPreferenceWritesForTesting();
  confirmState.value = { ...confirmState.value, visible: false };
});

describe('pickTheme', () => {
  it('sets a theme for both modes at once, with no confirm', async () => {
    await pickTheme(theme('mono', ['dark', 'light']));

    expect(confirmState.value.visible).toBe(false);
    expect(saved()).toEqual({ theme: 'mono' });
  });

  it('sets a single-mode theme at once when its mode is already painted', async () => {
    await pickTheme(theme('midnight', ['dark']));

    expect(confirmState.value.visible).toBe(false);
    expect(saved()).toEqual({ theme: 'midnight' });
  });

  it('switches to light mode after the confirm, for a light-only theme picked in dark', async () => {
    await answer(pickTheme(theme('paper', ['light'])), true);

    expect(saved()).toEqual({ theme: 'paper', 'theme-mode': 'light' });
    expect(paintedThemeMode.value).toBe('light');
  });

  it('switches to dark mode after the confirm, for a dark-only theme picked in light', async () => {
    applyThemeMode('light');
    await answer(pickTheme(theme('midnight', ['dark'])), true);

    expect(saved()).toEqual({ theme: 'midnight', 'theme-mode': 'dark' });
    expect(paintedThemeMode.value).toBe('dark');
  });

  it('changes nothing when the confirm is cancelled', async () => {
    await answer(pickTheme(theme('paper', ['light'])), false);

    expect(saved()).toEqual({});
    expect(paintedThemeMode.value).toBe('dark');
  });

  it('says the device stops following the system setting', async () => {
    preferences.value = { status: 'loaded', data: { 'theme-mode': 'system', theme: 'lucidos' } };
    const pick = pickTheme(theme('paper', ['light']));
    await vi.waitFor(() => expect(confirmState.value.visible).toBe(true));

    expect(confirmState.value.message).toContain('stops following the system setting');
    await answer(pick, false);
  });
});
