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

describe('settings search — Permissions section', () => {
  it('finds the Command Safety rows by name', () => {
    const guard = getSettingsSearchResults('command guard', 20);
    expect(guard.some((r) => r.id === 'command-safety:guard')).toBe(true);
    const judge = getSettingsSearchResults('judge model', 20);
    expect(judge.some((r) => r.id === 'command-safety:judge-model')).toBe(true);
  });

  it('finds both allowlist editors by name', () => {
    expect(getSettingsSearchResults('lucidos agent permissions', 20).some((r) => r.id === 'permissions:lucidos')).toBe(true);
    expect(getSettingsSearchResults('claude code permissions', 20).some((r) => r.id === 'permissions:claude-code')).toBe(true);
  });

  it('resolves a Command Safety entry to the permissions subview with its anchor', () => {
    const entry = findSettingsEntry('command-safety:guard');
    expect(entry?.subview).toBe('permissions');
    expect(entry?.anchor).toBe('command-safety:guard');
  });

  it('resolves the allowlist editors to their anchors under the permissions subview', () => {
    expect(findSettingsEntry('permissions:lucidos')?.subview).toBe('permissions');
    expect(findSettingsEntry('permissions:lucidos')?.anchor).toBe('permissions:lucidos');
    expect(findSettingsEntry('permissions:claude-code')?.anchor).toBe('permissions:claude-code');
  });
});

describe('settings search — System section', () => {
  it('finds the System page and its connection details', () => {
    expect(getSettingsSearchResults('system', 20).some((r) => r.id === 'system')).toBe(true);
    expect(getSettingsSearchResults('api url', 20).some((r) => r.id === 'system:connection')).toBe(true);
  });

  it('places Backup, Memory, and Disk Usage under the System breadcrumb', () => {
    expect(findSettingsEntry('backup')?.path).toBe('Settings → System');
    expect(findSettingsEntry('memory')?.path).toBe('Settings → System');
    expect(findSettingsEntry('disk-usage')?.path).toBe('Settings → System');
  });

  it('resolves maintenance to the System subview', () => {
    const entry = findSettingsEntry('system:maintenance');
    expect(entry?.subview).toBe('system');
    expect(entry?.anchor).toBe('system:maintenance');
  });
});
