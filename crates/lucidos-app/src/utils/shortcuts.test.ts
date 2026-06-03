import { describe, it, expect } from 'vitest';
import {
  SHORTCUT_DEFS,
  shortcutDef,
  normalizeKey,
  eventToBinding,
  matchesEvent,
  serializeBinding,
  parseBinding,
  formatBinding,
  isBindableChord,
  bindingsEqual,
  bindingSearchText,
  type Binding,
} from './shortcuts';

type Evt = Parameters<typeof eventToBinding>[0];
function evt(over: Partial<Evt>): Evt {
  return { metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: 'k', ...over };
}

describe('normalizeKey', () => {
  it('lowercases single chars and folds + to =', () => {
    expect(normalizeKey('O')).toBe('o');
    expect(normalizeKey('+')).toBe('=');
    expect(normalizeKey('Escape')).toBe('Escape');
  });
});

describe('eventToBinding / matchesEvent', () => {
  it('collapses Cmd or Ctrl into the mod modifier', () => {
    expect(eventToBinding(evt({ metaKey: true, key: 'k' }))).toEqual({ mod: true, shift: false, alt: false, key: 'k' });
    expect(eventToBinding(evt({ ctrlKey: true, key: 'k' }))).toEqual({ mod: true, shift: false, alt: false, key: 'k' });
  });

  it('Cmd+K and Ctrl+K both match a mod+k binding', () => {
    const b: Binding = { mod: true, shift: false, alt: false, key: 'k' };
    expect(matchesEvent(evt({ metaKey: true, key: 'k' }), b)).toBe(true);
    expect(matchesEvent(evt({ ctrlKey: true, key: 'k' }), b)).toBe(true);
  });

  it('does not match when shift differs or key differs', () => {
    const b: Binding = { mod: true, shift: false, alt: false, key: 'k' };
    expect(matchesEvent(evt({ ctrlKey: true, shiftKey: true, key: 'K' }), b)).toBe(false);
    expect(matchesEvent(evt({ ctrlKey: true, key: 'j' }), b)).toBe(false);
  });

  it('matches a shift chord with the uppercased key the browser reports', () => {
    const b: Binding = { mod: true, shift: true, alt: false, key: 'w' };
    expect(matchesEvent(evt({ ctrlKey: true, shiftKey: true, key: 'W' }), b)).toBe(true);
  });
});

describe('serializeBinding / parseBinding round-trip', () => {
  it('round-trips every default binding', () => {
    for (const def of SHORTCUT_DEFS) {
      const s = serializeBinding(def.defaultBinding);
      expect(parseBinding(s)).toEqual(def.defaultBinding);
    }
  });

  it('produces stable canonical strings', () => {
    expect(serializeBinding({ mod: true, shift: true, alt: false, key: 'o' })).toBe('mod+shift+o');
    expect(serializeBinding({ mod: true, shift: false, alt: false, key: '=' })).toBe('mod+=');
  });
});

describe('formatBinding', () => {
  it('Mac shows ⌃⇧O for the new-thread chord (Cmd+Shift+letter is OS-reserved)', () => {
    expect(formatBinding(shortcutDef('newThread').defaultBinding, true)).toBe('⌃⇧O');
  });
  it('Mac shows ⌃⇧S for search (Cmd+Shift+letter is OS-reserved → Control)', () => {
    expect(formatBinding(shortcutDef('searchEverywhere').defaultBinding, true)).toBe('⌃⇧S');
  });
  it('non-Mac shows Ctrl+Shift+O and Ctrl+Shift+S', () => {
    expect(formatBinding(shortcutDef('newThread').defaultBinding, false)).toBe('Ctrl+Shift+O');
    expect(formatBinding(shortcutDef('searchEverywhere').defaultBinding, false)).toBe('Ctrl+Shift+S');
  });
});

describe('isBindableChord', () => {
  it('accepts modifier chords', () => {
    expect(isBindableChord({ mod: true, shift: false, alt: false, key: 'k' })).toBe(true);
    expect(isBindableChord({ mod: false, shift: false, alt: true, key: 'k' })).toBe(true);
  });
  it('rejects a bare letter (would collide with type-to-focus)', () => {
    expect(isBindableChord({ mod: false, shift: true, alt: false, key: 'a' })).toBe(false);
  });
});

describe('bindingSearchText', () => {
  it('includes space- and plus-separated forms for both Ctrl and Cmd', () => {
    const text = bindingSearchText({ mod: true, shift: false, alt: false, key: 'k' });
    expect(text).toContain('ctrl k');
    expect(text).toContain('ctrl+k');
    expect(text).toContain('cmd k');
    expect(text).toContain('cmd+k');
  });
  it('includes shift in chord aliases', () => {
    const text = bindingSearchText({ mod: true, shift: true, alt: false, key: 'w' });
    expect(text).toContain('ctrl shift w');
    expect(text).toContain('ctrl+shift+w');
  });
});

describe('bindingsEqual', () => {
  it('compares all fields', () => {
    expect(bindingsEqual({ mod: true, shift: false, alt: false, key: 'k' }, { mod: true, shift: false, alt: false, key: 'k' })).toBe(true);
    expect(bindingsEqual({ mod: true, shift: false, alt: false, key: 'k' }, { mod: true, shift: true, alt: false, key: 'k' })).toBe(false);
  });
});
