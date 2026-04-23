import { isMac } from './platform';

const MOD = isMac ? '⌘' : 'Ctrl+';
const SHIFT = isMac ? '⇧' : 'Shift+';

const SHORTCUTS = {
  newThread: `${MOD}${SHIFT}O or C`,
  toggleThreadDrawer: 'T',
  searchEverywhere: `${MOD}K`,
  zoomIn: `${MOD}+`,
  zoomOut: `${MOD}−`,
};

type ShortcutName = keyof typeof SHORTCUTS;

export function shortcutHint(name: ShortcutName): string {
  return SHORTCUTS[name];
}

export function tooltipWithShortcut(text: string, name: ShortcutName): string {
  return `${text} · ${shortcutHint(name)}`;
}
