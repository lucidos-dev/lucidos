import { isMac } from './platform';

const MOD = isMac ? '⌘' : 'Ctrl+';

const SHORTCUTS = {
  // Ctrl on Mac (not Cmd): Cmd+Shift+O is intercepted system-side, only Ctrl actually fires.
  newThread: isMac ? '⌃⇧O' : 'Ctrl+Shift+O',
  toggleThreadDrawer: 'T',
  searchEverywhere: `${MOD}K`,
};

type ShortcutName = keyof typeof SHORTCUTS;

export function tooltipWithShortcut(text: string, name: ShortcutName): string {
  return `${text} · ${SHORTCUTS[name]}`;
}
