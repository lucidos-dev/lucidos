import { isMac } from './platform';

export type ShortcutCategory = 'Navigation' | 'View';

/** A single key combination, e.g. Cmd+K. */
export interface ShortcutBinding {
  /** Logical key tokens. Modifiers: 'cmd', 'shift', 'alt', 'ctrl'. Otherwise a single character. */
  keys: string[];
}

export interface ShortcutDef {
  category: ShortcutCategory;
  description: string;
  /** Multiple bindings render as "Ctrl+Shift+O or C". */
  bindings: ShortcutBinding[];
  /** Context note shown only in the cheat-sheet, omitted from tooltips. */
  note?: string;
}

export type ShortcutId =
  | 'searchEverywhere'
  | 'newThread'
  | 'closeFocusedThread'
  | 'toggleThreadDrawer'
  | 'zoomIn'
  | 'zoomOut'
  | 'resetZoom';

export const SHORTCUTS: Record<ShortcutId, ShortcutDef> = {
  searchEverywhere: {
    category: 'Navigation',
    description: 'Open search',
    bindings: [{ keys: ['cmd', 'k'] }],
  },
  newThread: {
    category: 'Navigation',
    description: 'Start a new thread',
    bindings: [{ keys: ['ctrl', 'shift', 'o'] }, { keys: ['c'] }],
    note: '"C" only fires when no text input is focused',
  },
  closeFocusedThread: {
    category: 'Navigation',
    description: 'Close focused thread (discard if draft, archive if active)',
    bindings: [{ keys: ['ctrl', 'shift', 'w'] }],
  },
  toggleThreadDrawer: {
    category: 'Navigation',
    description: 'Toggle thread drawer',
    bindings: [{ keys: ['t'] }],
    note: 'Only fires when no text input is focused',
  },
  zoomIn: {
    category: 'View',
    description: 'Increase UI scale',
    bindings: [{ keys: ['cmd', '+'] }],
  },
  zoomOut: {
    category: 'View',
    description: 'Decrease UI scale',
    bindings: [{ keys: ['cmd', '-'] }],
  },
  resetZoom: {
    category: 'View',
    description: 'Reset UI scale',
    bindings: [{ keys: ['cmd', '0'] }],
  },
};

const MOD_TOKEN = isMac ? '⌘' : 'Ctrl';
const SHIFT_TOKEN = isMac ? '⇧' : 'Shift';
const ALT_TOKEN = isMac ? '⌥' : 'Alt';
const CTRL_TOKEN = isMac ? '⌃' : 'Ctrl';
const JOIN = isMac ? '' : '+';

export function formatKey(key: string): string {
  switch (key) {
    case 'cmd': return MOD_TOKEN;
    case 'shift': return SHIFT_TOKEN;
    case 'alt': return ALT_TOKEN;
    case 'ctrl': return CTRL_TOKEN;
    default: return key.toUpperCase();
  }
}

export function formatBinding(binding: ShortcutBinding): string {
  return binding.keys.map(formatKey).join(JOIN);
}

export function tooltipWithShortcut(text: string, id: ShortcutId): string {
  const def = SHORTCUTS[id];
  const formatted = def.bindings.map(formatBinding).join(' or ');
  return `${text} · ${formatted}`;
}
