// Keyboard-shortcut registry + pure binding logic. No store/DOM dependency so
// it stays unit-testable. The override-aware layer (reading the user's custom
// bindings from preferences, saving them, override-aware tooltips) lives in
// `store/actions/keybindings.ts`; the dispatcher that runs them lives in
// `hooks/useKeyboardShortcuts.ts`.

export type ShortcutId =
  | 'newThread'
  | 'closeThread'
  | 'searchEverywhere'
  | 'searchFiles'
  | 'openSettings'
  | 'showShortcuts'
  | 'openNotifications'
  | 'focusComposer'
  | 'stopThread'
  | 'askSideQuestion'
  | 'copyLastResponse'
  | 'renameThread'
  | 'followLiveEdge'
  | 'toggleCall'
  | 'openAgentMenu'
  | 'showThreadDiff'
  | 'applyChange'
  | 'focusNewestToast'
  | 'openThreadActions'
  | 'toggleSubthreads'
  | 'toggleThreadFilter'
  | 'searchThreads'
  | 'toggleMenuDrawer'
  | 'historyBack'
  | 'historyForward'
  | 'prevThreadInList'
  | 'nextThreadInList'
  | 'prevTurnOrNotification'
  | 'nextTurnOrNotification'
  | 'toggleThreadDrawer'
  | 'toggleThreadPane'
  | 'toggleContentPane'
  | 'maximizePaneGroup'
  | 'narrowThreadPane'
  | 'widenThreadPane'
  | 'narrowThreadDrawer'
  | 'widenThreadDrawer'
  | 'resetPaneLayout'
  | 'refreshPanel'
  | 'toggleAppFullscreen'
  | 'findInView'
  | 'toggleSourceView'
  | 'toggleLineWrap'
  | 'zoomIn'
  | 'zoomOut'
  | 'zoomReset';

export type ShortcutCategory = 'Navigation' | 'Thread' | 'Panes' | 'View';

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
  /** How to remember the DEFAULT chord: a letter's word, or the app it is
   *  borrowed from. Platform-neutral, so it names no modifier glyph. */
  mnemonic?: string;
  /** Runs only from a keydown on the host document. A chord forwarded from an
   *  app frame or the HTML preview is refused, since that frame's own script
   *  can forge one. Set on anything that acts beyond the screen: merging a
   *  change, or opening the microphone. */
  hostOnly?: boolean;
}

const B = (mod: boolean, shift: boolean, alt: boolean, key: string): Binding => ({ mod, shift, alt, key });

/** The full registry. Order is the cheat-sheet display order. All entries are
 *  rebindable; the single-key `c`/`t` shortcuts were intentionally dropped.
 *  A default copies the chord well-known apps use for the same action, where
 *  one exists (docs/plans/2026-09-29-familiar-keyboard-shortcuts.md).
 *  Labels render in Settings → Keyboard shortcuts (and in Search Everywhere via
 *  `searchIndex.ts`), so they use `system-knowhow/glossary.md`'s two layers: a
 *  shortcut acting on ONE pane names it mechanically — thread drawer, thread
 *  pane, content pane — while `maximizePaneGroup`, which acts on a whole side,
 *  names the sides (Conversation / Canvas). Never surface the dev-only "pane
 *  group" here; it is defined in `docs/glossary.md` only. */
export const SHORTCUT_DEFS: readonly ShortcutDef[] = [
  { id: 'newThread', label: 'New thread', category: 'Navigation', defaultBinding: B(true, true, false, 'o'), mnemonic: 'ChatGPT\'s new chat chord' },
  { id: 'closeThread', label: 'Close thread (cascade)', category: 'Navigation', defaultBinding: B(true, true, false, 'w'), mnemonic: 'W, as in close window' },
  { id: 'searchEverywhere', label: 'Search everywhere', category: 'Navigation', defaultBinding: B(true, false, false, 'k'), mnemonic: 'The search chord in Slack, Linear and Notion' },
  { id: 'searchFiles', label: 'Search files', category: 'Navigation', defaultBinding: B(true, false, false, 'p'), mnemonic: 'Quick open in VS Code and Zed' },
  { id: 'openSettings', label: 'Open settings', category: 'Navigation', defaultBinding: B(true, false, false, ','), mnemonic: 'Settings in every Mac app' },
  { id: 'showShortcuts', label: 'Show keyboard shortcuts', category: 'Navigation', defaultBinding: B(true, false, false, '/'), mnemonic: 'The shortcut list in ChatGPT and Slack' },
  { id: 'openNotifications', label: 'Open notifications', category: 'Navigation', defaultBinding: B(true, true, false, 'i'), mnemonic: 'I for Inbox' },
  { id: 'focusNewestToast', label: 'Focus newest toast', category: 'Navigation', defaultBinding: B(true, true, false, 'n'), mnemonic: 'N for Notice' },
  { id: 'openThreadActions', label: 'Open thread actions (highlighted drawer row, else open thread)', category: 'Navigation', defaultBinding: B(true, true, false, 'm'), mnemonic: 'M for Menu' },
  { id: 'toggleSubthreads', label: 'Expand or collapse sub-threads (focused thread)', category: 'Navigation', defaultBinding: B(true, true, false, 'e'), mnemonic: 'E for Expand' },
  { id: 'toggleThreadFilter', label: 'Show the thread filter or the thread list', category: 'Navigation', defaultBinding: B(true, true, false, 'u') },
  { id: 'searchThreads', label: 'Search threads (thread drawer)', category: 'Navigation', defaultBinding: B(true, true, false, 'k'), mnemonic: 'Search everywhere\'s K, for threads only' },
  { id: 'toggleMenuDrawer', label: 'Open or close the menu drawer', category: 'Navigation', defaultBinding: B(true, true, false, 'x') },
  { id: 'historyBack', label: 'Back (focused pane)', category: 'Navigation', defaultBinding: B(true, false, true, 'ArrowDown') },
  { id: 'historyForward', label: 'Forward (focused pane)', category: 'Navigation', defaultBinding: B(true, false, true, 'ArrowUp') },
  { id: 'prevThreadInList', label: 'Previous thread in the open list or section (thread drawer)', category: 'Navigation', defaultBinding: B(false, false, true, 'ArrowUp'), mnemonic: 'Slack\'s previous channel chord' },
  { id: 'nextThreadInList', label: 'Next thread in the open list or section (thread drawer)', category: 'Navigation', defaultBinding: B(false, false, true, 'ArrowDown'), mnemonic: 'Slack\'s next channel chord' },
  { id: 'prevTurnOrNotification', label: 'Previous turn (thread) or newer notification', category: 'Navigation', defaultBinding: B(true, false, false, 'ArrowUp') },
  { id: 'nextTurnOrNotification', label: 'Next turn (thread) or older notification', category: 'Navigation', defaultBinding: B(true, false, false, 'ArrowDown') },
  { id: 'focusComposer', label: 'Focus the composer', category: 'Thread', defaultBinding: B(false, true, false, 'Escape'), mnemonic: 'Focus input in ChatGPT' },
  { id: 'stopThread', label: 'Stop the running thread', category: 'Thread', defaultBinding: B(true, false, false, '.'), mnemonic: 'The classic Mac Cancel' },
  { id: 'askSideQuestion', label: 'Side question mode (on or off)', category: 'Thread', defaultBinding: B(false, false, true, 'Enter'), mnemonic: 'Alt for an aside' },
  { id: 'copyLastResponse', label: 'Copy last response', category: 'Thread', defaultBinding: B(true, true, false, 'c'), mnemonic: 'C for Copy, as in ChatGPT' },
  { id: 'renameThread', label: 'Rename thread', category: 'Thread', defaultBinding: B(false, false, false, 'F2'), mnemonic: 'Rename in Windows Explorer and VS Code' },
  { id: 'followLiveEdge', label: 'Follow the live edge (arm or disarm)', category: 'Thread', defaultBinding: B(true, true, false, 'l'), mnemonic: 'L for Live' },
  { id: 'toggleCall', label: 'Start or end a voice call', category: 'Thread', defaultBinding: B(true, true, false, 'h'), mnemonic: 'Slack\'s huddle chord', hostOnly: true },
  { id: 'openAgentMenu', label: 'Open the agent menu (model and effort)', category: 'Thread', defaultBinding: B(true, true, false, 'g'), mnemonic: 'G for aGent' },
  { id: 'showThreadDiff', label: 'Show what the thread changed', category: 'Thread', defaultBinding: B(true, true, false, 'd'), mnemonic: 'D for Diff' },
  { id: 'applyChange', label: 'Apply the thread\'s change, or apply on settle', category: 'Thread', defaultBinding: B(true, true, false, 'a'), mnemonic: 'A for Apply', hostOnly: true },
  { id: 'toggleThreadDrawer', label: 'Show or hide thread drawer', category: 'Panes', defaultBinding: B(true, true, false, '1'), mnemonic: 'Panes count 1, 2, 3 from the left' },
  { id: 'toggleThreadPane', label: 'Focus or hide thread pane', category: 'Panes', defaultBinding: B(true, true, false, '2'), mnemonic: 'Panes count 1, 2, 3 from the left' },
  { id: 'toggleContentPane', label: 'Focus or hide content pane', category: 'Panes', defaultBinding: B(true, true, false, '3'), mnemonic: 'Panes count 1, 2, 3 from the left' },
  { id: 'maximizePaneGroup', label: 'Maximize focused side (Conversation or Canvas)', category: 'Panes', defaultBinding: B(true, true, false, 'Enter') },
  { id: 'narrowThreadPane', label: 'Narrow thread pane', category: 'Panes', defaultBinding: B(true, false, true, 'ArrowLeft') },
  { id: 'widenThreadPane', label: 'Widen thread pane', category: 'Panes', defaultBinding: B(true, false, true, 'ArrowRight') },
  { id: 'narrowThreadDrawer', label: 'Narrow thread drawer', category: 'Panes', defaultBinding: B(true, true, true, 'ArrowLeft') },
  { id: 'widenThreadDrawer', label: 'Widen thread drawer', category: 'Panes', defaultBinding: B(true, true, true, 'ArrowRight') },
  { id: 'resetPaneLayout', label: 'Reset pane layout', category: 'Panes', defaultBinding: B(true, false, true, '0'), mnemonic: '0, as in Reset zoom' },
  { id: 'refreshPanel', label: 'Refresh the content pane', category: 'View', defaultBinding: B(true, false, false, 'r'), mnemonic: 'Reload in every browser' },
  { id: 'toggleAppFullscreen', label: 'Fullscreen the open app', category: 'View', defaultBinding: B(true, true, false, 'f'), mnemonic: 'F for Fullscreen' },
  { id: 'findInView', label: 'Find in the focused pane (app, file or thread)', category: 'View', defaultBinding: B(true, false, false, 'f'), mnemonic: 'Find in every browser' },
  { id: 'toggleSourceView', label: 'Show source or rendered (file preview)', category: 'View', defaultBinding: B(true, true, false, 's'), mnemonic: 'S for Source' },
  { id: 'toggleLineWrap', label: 'Wrap long lines (source view)', category: 'View', defaultBinding: B(true, true, false, 'b'), mnemonic: 'B for line Break' },
  { id: 'zoomIn', label: 'Zoom in', category: 'View', defaultBinding: B(true, false, false, '='), mnemonic: 'Browser zoom' },
  { id: 'zoomOut', label: 'Zoom out', category: 'View', defaultBinding: B(true, false, false, '-'), mnemonic: 'Browser zoom' },
  { id: 'zoomReset', label: 'Reset zoom', category: 'View', defaultBinding: B(true, false, false, '0'), mnemonic: 'Browser zoom' },
] as const;

export function shortcutDef(id: ShortcutId): ShortcutDef {
  const def = SHORTCUT_DEFS.find((d) => d.id === id);
  if (!def) throw new Error(`Unknown shortcut id: ${id}`);
  return def;
}

/** The `data-search-anchor` on a shortcut's Settings row, which its Search
 *  Everywhere result scrolls to. */
export function shortcutSearchAnchor(id: ShortcutId): string {
  return `shortcut:${id}`;
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
  Escape: 'Esc',
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
