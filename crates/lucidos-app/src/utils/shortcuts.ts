// Keyboard-shortcut registry + pure binding logic. No store/DOM dependency so
// it stays unit-testable. The override-aware layer (reading the user's custom
// bindings from preferences, saving them, override-aware tooltips) lives in
// `store/actions/keybindings.ts`; the dispatcher that runs them lives in
// `hooks/useKeyboardShortcuts.ts`.

export type ShortcutId =
  | 'newThread'
  | 'closeThread'
  | 'searchEverywhere'
  | 'openThreadActions'
  | 'toggleSubthreads'
  | 'historyBack'
  | 'historyForward'
  | 'prevThreadTurn'
  | 'nextThreadTurn'
  | 'toggleThreadDrawer'
  | 'toggleThreadPane'
  | 'toggleContentPane'
  | 'maximizePaneGroup'
  | 'narrowThreadPane'
  | 'widenThreadPane'
  | 'narrowThreadDrawer'
  | 'widenThreadDrawer'
  | 'resetPaneLayout'
  | 'zoomIn'
  | 'zoomOut'
  | 'zoomReset';

export type ShortcutCategory = 'Navigation' | 'Panes' | 'View';

/** A normalized key chord. `mod` means "the platform primary modifier" and
 *  matches either Cmd (meta) or Ctrl — the same lenient rule the handlers have
 *  always used, so `Cmd+K` and `Ctrl+K` both fire. `key` is normalized to a
 *  single lowercase token. */
export interface Binding {
  mod: boolean;
  shift: boolean;
  alt: boolean;
  key: string;
}

export interface ShortcutDef {
  id: ShortcutId;
  label: string;
  category: ShortcutCategory;
  defaultBinding: Binding;
}

const B = (mod: boolean, shift: boolean, alt: boolean, key: string): Binding => ({ mod, shift, alt, key });

/** The full registry. Order is the cheat-sheet display order. All entries are
 *  rebindable; the single-key `c`/`t` shortcuts were intentionally dropped. */
export const SHORTCUT_DEFS: readonly ShortcutDef[] = [
  { id: 'newThread', label: 'New thread', category: 'Navigation', defaultBinding: B(true, true, false, 'o') },
  { id: 'closeThread', label: 'Close thread (cascade)', category: 'Navigation', defaultBinding: B(true, true, false, 'w') },
  { id: 'searchEverywhere', label: 'Search everywhere', category: 'Navigation', defaultBinding: B(true, true, false, 's') },
  { id: 'openThreadActions', label: 'Open thread actions (highlighted drawer row)', category: 'Navigation', defaultBinding: B(true, true, false, 'm') },
  { id: 'toggleSubthreads', label: 'Expand or collapse sub-threads (focused thread)', category: 'Navigation', defaultBinding: B(true, true, false, 'e') },
  { id: 'historyBack', label: 'Back (focused pane)', category: 'Navigation', defaultBinding: B(true, false, true, 'ArrowDown') },
  { id: 'historyForward', label: 'Forward (focused pane)', category: 'Navigation', defaultBinding: B(true, false, true, 'ArrowUp') },
  { id: 'prevThreadTurn', label: 'Previous turn (conversation)', category: 'Navigation', defaultBinding: B(true, false, false, 'ArrowUp') },
  { id: 'nextThreadTurn', label: 'Next turn (conversation)', category: 'Navigation', defaultBinding: B(true, false, false, 'ArrowDown') },
  { id: 'toggleThreadDrawer', label: 'Show or hide thread drawer', category: 'Panes', defaultBinding: B(true, true, false, '1') },
  { id: 'toggleThreadPane', label: 'Focus or hide thread pane', category: 'Panes', defaultBinding: B(true, true, false, '2') },
  { id: 'toggleContentPane', label: 'Focus or hide content pane', category: 'Panes', defaultBinding: B(true, true, false, '3') },
  { id: 'maximizePaneGroup', label: 'Maximize focused pane group', category: 'Panes', defaultBinding: B(true, true, false, 'Enter') },
  { id: 'narrowThreadPane', label: 'Narrow thread pane', category: 'Panes', defaultBinding: B(true, false, true, 'ArrowLeft') },
  { id: 'widenThreadPane', label: 'Widen thread pane', category: 'Panes', defaultBinding: B(true, false, true, 'ArrowRight') },
  { id: 'narrowThreadDrawer', label: 'Narrow thread drawer', category: 'Panes', defaultBinding: B(true, true, true, 'ArrowLeft') },
  { id: 'widenThreadDrawer', label: 'Widen thread drawer', category: 'Panes', defaultBinding: B(true, true, true, 'ArrowRight') },
  { id: 'resetPaneLayout', label: 'Reset pane layout', category: 'Panes', defaultBinding: B(true, false, true, '0') },
  { id: 'zoomIn', label: 'Zoom in', category: 'View', defaultBinding: B(true, false, false, '=') },
  { id: 'zoomOut', label: 'Zoom out', category: 'View', defaultBinding: B(true, false, false, '-') },
  { id: 'zoomReset', label: 'Reset zoom', category: 'View', defaultBinding: B(true, false, false, '0') },
] as const;

export function shortcutDef(id: ShortcutId): ShortcutDef {
  const def = SHORTCUT_DEFS.find((d) => d.id === id);
  if (!def) throw new Error(`Unknown shortcut id: ${id}`);
  return def;
}

/** Normalize a `KeyboardEvent.key` to the registry's canonical token: single
 *  characters lowercased, and `+` folded to `=` so zoom-in matches whether or
 *  not Shift produced the literal plus. */
export function normalizeKey(key: string): string {
  if (key === '+') return '=';
  return key.length === 1 ? key.toLowerCase() : key;
}

/** Capture the chord a user just pressed. `mod` collapses Cmd/Ctrl into one
 *  primary modifier (matching the dispatcher's lenient rule). */
export function eventToBinding(
  e: Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key'>,
): Binding {
  return {
    mod: e.metaKey || e.ctrlKey,
    shift: e.shiftKey,
    alt: e.altKey,
    key: normalizeKey(e.key),
  };
}

/** Does a keydown event match a binding? Cmd and Ctrl are interchangeable for
 *  the `mod` modifier (both fire), preserving the handlers' historical leniency. */
export function matchesEvent(
  e: Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key'>,
  b: Binding,
): boolean {
  return (
    (e.metaKey || e.ctrlKey) === b.mod &&
    e.shiftKey === b.shift &&
    e.altKey === b.alt &&
    normalizeKey(e.key) === b.key
  );
}

export function bindingsEqual(a: Binding, b: Binding): boolean {
  return a.mod === b.mod && a.shift === b.shift && a.alt === b.alt && a.key === b.key;
}

/** A keypress is a valid shortcut only if it carries a non-Shift modifier (so a
 *  bare letter — which would collide with type-to-focus — can't be bound) or a
 *  named key (Escape, F-keys are excluded elsewhere). Used by the recorder to
 *  reject unusable chords. */
export function isBindableChord(b: Binding): boolean {
  if (b.mod || b.alt) return true;
  // No primary modifier: only allow it if the key isn't a lone printable char.
  return b.key.length > 1;
}

/** Canonical storage form, e.g. `mod+shift+o`, `mod+k`, `mod+=`. Stable across
 *  platforms; the display form is derived separately by `formatBinding`. */
export function serializeBinding(b: Binding): string {
  const parts: string[] = [];
  if (b.mod) parts.push('mod');
  if (b.shift) parts.push('shift');
  if (b.alt) parts.push('alt');
  parts.push(b.key);
  return parts.join('+');
}

export function parseBinding(s: string): Binding | null {
  const parts = s.split('+');
  // The key is the last segment; `+` itself can't be a segment because we fold
  // it to `=` before serializing, so a trailing empty segment never happens.
  const key = parts.pop();
  if (!key) return null;
  const set = new Set(parts);
  return {
    mod: set.has('mod'),
    shift: set.has('shift'),
    alt: set.has('alt'),
    key: normalizeKey(key),
  };
}

/** Free-text aliases a user might type to find this binding in search, e.g.
 *  `ctrl k ctrl+k cmd k cmd+k`. Covers both `mod` spellings (Ctrl/Cmd, since
 *  either fires) and both space- and plus-separated forms, so a query like
 *  "ctrl k" matches a `⌘K` binding. */
export function bindingSearchText(b: Binding): string {
  const primaries = b.mod ? ['ctrl', 'cmd'] : [''];
  const mid: string[] = [];
  if (b.shift) mid.push('shift');
  if (b.alt) mid.push('alt');
  const out: string[] = [];
  for (const p of primaries) {
    const seq = [p, ...mid, b.key].filter(Boolean);
    out.push(seq.join(' '));
    out.push(seq.join('+'));
  }
  return out.join(' ').toLowerCase();
}

/** Named keys whose display form is a glyph rather than the raw
 *  `KeyboardEvent.key` token. */
const KEY_GLYPHS: Record<string, string> = {
  ArrowLeft: '←',
  ArrowRight: '→',
  ArrowUp: '↑',
  ArrowDown: '↓',
  Enter: '↵',
};

/** Platform-correct display string, e.g. Mac `⌘K` / `⌃⇧O`, Win/Linux
 *  `Ctrl+K` / `Ctrl+Shift+O`. On Mac a `mod+Shift+<letter-or-digit>` chord
 *  renders with the Control glyph (⌃) rather than Command (⌘): the OS reserves
 *  Cmd+Shift+<letter> (and the Cmd+Shift+3/4/5 screenshot digits), so Ctrl is
 *  the modifier that actually fires — matching the long-standing display for
 *  the New-thread chord. The rule covers digits too so the three pane toggles
 *  read uniformly. Named keys display as glyphs (←→↑↓). */
export function formatBinding(b: Binding, isMac: boolean): string {
  const isAlnum = /^[a-z0-9]$/.test(b.key);
  const macUsesCtrl = b.mod && b.shift && isAlnum;
  const keyDisplay = KEY_GLYPHS[b.key] ?? (b.key.length === 1 ? b.key.toUpperCase() : b.key);
  if (isMac) {
    let out = '';
    if (b.mod) out += macUsesCtrl ? '⌃' : '⌘';
    if (b.alt) out += '⌥';
    if (b.shift) out += '⇧';
    return out + keyDisplay;
  }
  const parts: string[] = [];
  if (b.mod) parts.push('Ctrl');
  if (b.alt) parts.push('Alt');
  if (b.shift) parts.push('Shift');
  parts.push(keyDisplay);
  return parts.join('+');
}
