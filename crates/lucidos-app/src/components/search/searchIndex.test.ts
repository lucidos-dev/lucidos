import { describe, it, expect, beforeEach } from 'vitest';
import { preferences } from '../../store/store';
import { getSettingsSearchResults, findSettingsEntry } from './searchIndex';

beforeEach(() => {
  // Default bindings (no overrides) — searchEverywhere = mod+Shift+S.
  preferences.value = { status: 'not-loaded' };
});

describe('settings search — keyboard shortcuts', () => {
  it('finds a shortcut by its key combo typed with a space ("ctrl shift s")', () => {
    const results = getSettingsSearchResults('ctrl shift s', 20);
    expect(results.some((r) => r.id === 'shortcut:searchEverywhere')).toBe(true);
  });

  it('finds a shortcut by the plus form ("ctrl+shift+w")', () => {
    const results = getSettingsSearchResults('ctrl+shift+w', 20);
    expect(results.some((r) => r.id === 'shortcut:closeThread')).toBe(true);
  });

  it('finds the cheat sheet by name', () => {
    const results = getSettingsSearchResults('keyboard', 20);
    expect(results.some((r) => r.id === 'keyboard-shortcuts')).toBe(true);
  });

  it('resolves a synthesized shortcut entry to the keyboard-shortcuts subview', () => {
    const entry = findSettingsEntry('shortcut:searchEverywhere');
    expect(entry?.subview).toBe('keyboard-shortcuts');
  });

  it('reflects a custom binding in search (rebound search to ctrl+shift+p)', () => {
    preferences.value = { status: 'loaded', data: { keybindings: JSON.stringify({ searchEverywhere: 'mod+shift+p' }) } };
    expect(getSettingsSearchResults('ctrl shift p', 20).some((r) => r.id === 'shortcut:searchEverywhere')).toBe(true);
    // The old default combo no longer matches it.
    expect(getSettingsSearchResults('ctrl shift s', 20).some((r) => r.id === 'shortcut:searchEverywhere')).toBe(false);
  });
});
