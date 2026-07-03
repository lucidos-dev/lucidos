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
  it('Mac shows ⌃⇧3 for a mod+shift+digit pane toggle (Cmd+Shift+digit hits the screenshot chords)', () => {
    expect(formatBinding(shortcutDef('toggleContentPane').defaultBinding, true)).toBe('⌃⇧3');
    expect(formatBinding(shortcutDef('toggleContentPane').defaultBinding, false)).toBe('Ctrl+Shift+3');
  });
  it('renders arrow keys as glyphs on both platforms', () => {
    expect(formatBinding(shortcutDef('widenThreadPane').defaultBinding, true)).toBe('⌘⌥→');
    expect(formatBinding(shortcutDef('narrowThreadPane').defaultBinding, false)).toBe('Ctrl+Alt+←');
    expect(formatBinding(shortcutDef('historyBack').defaultBinding, true)).toBe('⌘⌥↓');
    expect(formatBinding(shortcutDef('narrowThreadDrawer').defaultBinding, false)).toBe('Ctrl+Alt+Shift+←');
  });
  it('renders Enter as the ↵ glyph and keeps ⌘ (Enter is not an OS-reserved alnum chord)', () => {
    expect(formatBinding(shortcutDef('maximizePaneGroup').defaultBinding, true)).toBe('⌘⇧↵');
    expect(formatBinding(shortcutDef('maximizePaneGroup').defaultBinding, false)).toBe('Ctrl+Shift+↵');
  });
  it('renders the mod+arrow turn shortcuts as ⌘↑ / ⌘↓ (arrows are not OS-reserved alnum chords)', () => {
    expect(formatBinding(shortcutDef('prevThreadTurn').defaultBinding, true)).toBe('⌘↑');
    expect(formatBinding(shortcutDef('nextThreadTurn').defaultBinding, true)).toBe('⌘↓');
    expect(formatBinding(shortcutDef('nextThreadTurn').defaultBinding, false)).toBe('Ctrl+↓');
  });
});

describe('SHORTCUT_DEFS registry invariants', () => {
  it('no two shortcuts share a default binding', () => {
    const serialized = SHORTCUT_DEFS.map((d) => serializeBinding(d.defaultBinding));
    expect(new Set(serialized).size).toBe(SHORTCUT_DEFS.length);
  });
  it('every default binding is a bindable chord', () => {
    for (const def of SHORTCUT_DEFS) {
      expect(isBindableChord(def.defaultBinding), def.id).toBe(true);
    }
  });
  it('exposes turn-by-turn conversation shortcuts on the mod+arrow keys', () => {
    // ⌘↑ / ⌘↓ step the transcript one turn (a .chat-exchange) at a time. Free of
    // the mod+alt+arrow history shortcuts (alt differs) — the collision invariant
    // above also covers this.
    expect(shortcutDef('prevThreadTurn').defaultBinding).toEqual({ mod: true, shift: false, alt: false, key: 'ArrowUp' });
    expect(shortcutDef('nextThreadTurn').defaultBinding).toEqual({ mod: true, shift: false, alt: false, key: 'ArrowDown' });
    expect(shortcutDef('prevThreadTurn').category).toBe('Navigation');
    expect(shortcutDef('nextThreadTurn').category).toBe('Navigation');
  });
  it('exposes the customizable "Open thread actions" drawer shortcut', () => {
    // The keyboard route to a drawer row's ⋯ menu is a first-class registry entry
    // (so it is documented + rebindable in Settings → Keyboard Shortcuts), not a
    // hand-rolled key. The collision + bindable invariants above also cover it.
    const def = shortcutDef('openThreadActions');
    expect(def.category).toBe('Navigation');
    expect(def.defaultBinding).toEqual({ mod: true, shift: true, alt: false, key: 'm' });
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
