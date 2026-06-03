import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('./preferences', () => ({ savePreference: vi.fn(async () => {}) }));

import { preferences } from '../store';
import { savePreference } from './preferences';
import {
  bindingFor,
  isCustomized,
  matchShortcut,
  setBinding,
  resetBinding,
  tooltipWithShortcut,
  recordChord,
  KEYBINDINGS_PREF_KEY,
} from './keybindings';

type Evt = Parameters<typeof matchShortcut>[0];
function evt(over: Partial<Evt>): Evt {
  return { metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: 'k', ...over };
}
function setPrefs(map: Record<string, string> | null) {
  preferences.value = {
    status: 'loaded',
    data: map ? { [KEYBINDINGS_PREF_KEY]: JSON.stringify(map) } : {},
  };
}

beforeEach(() => {
  preferences.value = { status: 'not-loaded' };
  vi.mocked(savePreference).mockClear();
});

describe('bindingFor', () => {
  it('returns the default when preferences are not loaded or no override exists', () => {
    expect(bindingFor('searchEverywhere')).toEqual({ mod: true, shift: true, alt: false, key: 's' });
    setPrefs({});
    expect(bindingFor('newThread')).toEqual({ mod: true, shift: true, alt: false, key: 'o' });
  });

  it('returns the override when one is persisted', () => {
    setPrefs({ searchEverywhere: 'mod+shift+p' });
    expect(bindingFor('searchEverywhere')).toEqual({ mod: true, shift: true, alt: false, key: 'p' });
  });

  it('falls back to default on corrupt JSON', () => {
    preferences.value = { status: 'loaded', data: { [KEYBINDINGS_PREF_KEY]: '{not json' } };
    expect(bindingFor('searchEverywhere')).toEqual({ mod: true, shift: true, alt: false, key: 's' });
  });
});

describe('isCustomized', () => {
  it('is false for defaults, true for an override', () => {
    setPrefs({});
    expect(isCustomized('searchEverywhere')).toBe(false);
    setPrefs({ searchEverywhere: 'mod+shift+p' });
    expect(isCustomized('searchEverywhere')).toBe(true);
  });
});

describe('matchShortcut', () => {
  it('finds the shortcut whose current binding matches the event', () => {
    setPrefs({});
    expect(matchShortcut(evt({ ctrlKey: true, shiftKey: true, key: 'S' }))).toBe('searchEverywhere');
    expect(matchShortcut(evt({ ctrlKey: true, shiftKey: true, key: 'W' }))).toBe('closeThread');
  });

  it('honors overrides', () => {
    setPrefs({ searchEverywhere: 'mod+shift+p' });
    expect(matchShortcut(evt({ ctrlKey: true, shiftKey: true, key: 'S' }))).toBeNull();
    expect(matchShortcut(evt({ ctrlKey: true, shiftKey: true, key: 'P' }))).toBe('searchEverywhere');
  });

  it('can exclude one id (the one being rebound)', () => {
    setPrefs({});
    expect(matchShortcut(evt({ ctrlKey: true, shiftKey: true, key: 'S' }), 'searchEverywhere')).toBeNull();
  });
});

describe('setBinding / resetBinding', () => {
  it('persists only non-default bindings as a JSON map', async () => {
    setPrefs({});
    await setBinding('searchEverywhere', { mod: true, shift: true, alt: false, key: 'p' });
    expect(savePreference).toHaveBeenCalledWith(KEYBINDINGS_PREF_KEY, JSON.stringify({ searchEverywhere: 'mod+shift+p' }));
  });

  it('keeps existing overrides when changing a different shortcut', async () => {
    setPrefs({ newThread: 'mod+shift+n' });
    await setBinding('searchEverywhere', { mod: true, shift: false, alt: false, key: 'j' });
    expect(savePreference).toHaveBeenCalledWith(
      KEYBINDINGS_PREF_KEY,
      JSON.stringify({ newThread: 'mod+shift+n', searchEverywhere: 'mod+j' }),
    );
  });

  it('resetBinding drops the id from the persisted map', async () => {
    setPrefs({ searchEverywhere: 'mod+shift+p', newThread: 'mod+shift+n' });
    await resetBinding('searchEverywhere');
    expect(savePreference).toHaveBeenCalledWith(KEYBINDINGS_PREF_KEY, JSON.stringify({ newThread: 'mod+shift+n' }));
  });
});

describe('recordChord', () => {
  beforeEach(() => setPrefs({}));

  it('keeps listening on a bare modifier press', () => {
    expect(recordChord(evt({ ctrlKey: true, key: 'Control' }), 'newThread')).toEqual({ kind: 'modifier' });
  });

  it('cancels on Escape', () => {
    expect(recordChord(evt({ key: 'Escape' }), 'newThread')).toEqual({ kind: 'cancel' });
  });

  it('rejects an unbindable bare letter', () => {
    expect(recordChord(evt({ shiftKey: true, key: 'A' }), 'newThread')).toEqual({ kind: 'invalid' });
  });

  it('reports a conflict with a different shortcut', () => {
    // Recording newThread, but Ctrl+Shift+S is searchEverywhere's binding.
    expect(recordChord(evt({ ctrlKey: true, shiftKey: true, key: 'S' }), 'newThread')).toEqual({ kind: 'conflict', withId: 'searchEverywhere' });
  });

  it('accepts a free chord', () => {
    expect(recordChord(evt({ ctrlKey: true, altKey: true, key: 'n' }), 'newThread')).toEqual({
      kind: 'ok',
      binding: { mod: true, shift: false, alt: true, key: 'n' },
    });
  });

  it('re-recording a shortcut to its own current binding is not a conflict', () => {
    expect(recordChord(evt({ ctrlKey: true, shiftKey: true, key: 'S' }), 'searchEverywhere')).toEqual({
      kind: 'ok',
      binding: { mod: true, shift: true, alt: false, key: 's' },
    });
  });
});

describe('tooltipWithShortcut', () => {
  it('reflects the current binding (non-Mac form in the test env)', () => {
    setPrefs({});
    expect(tooltipWithShortcut('Search', 'searchEverywhere')).toBe('Search · Ctrl+Shift+S');
    setPrefs({ searchEverywhere: 'mod+shift+p' });
    expect(tooltipWithShortcut('Search', 'searchEverywhere')).toBe('Search · Ctrl+Shift+P');
  });
});
