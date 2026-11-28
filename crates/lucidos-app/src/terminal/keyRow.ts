export interface KeyRowKey {
  id: string;
  label: string;
  ariaLabel: string;
  bytes?: string;
}

export const CTRL_KEY_ID = 'ctrl';
export const PASTE_KEY_ID = 'paste';

export const KEY_ROW: readonly KeyRowKey[] = [
  { id: 'esc', label: 'Esc', ariaLabel: 'Escape', bytes: '\x1b' },
  { id: 'tab', label: 'Tab', ariaLabel: 'Tab', bytes: '\t' },
  { id: CTRL_KEY_ID, label: 'Ctrl', ariaLabel: 'Control' },
  { id: 'left', label: '←', ariaLabel: 'Left arrow', bytes: '\x1b[D' },
  { id: 'up', label: '↑', ariaLabel: 'Up arrow', bytes: '\x1b[A' },
  { id: 'down', label: '↓', ariaLabel: 'Down arrow', bytes: '\x1b[B' },
  { id: 'right', label: '→', ariaLabel: 'Right arrow', bytes: '\x1b[C' },
  { id: 'pipe', label: '|', ariaLabel: 'Pipe', bytes: '|' },
  { id: 'tilde', label: '~', ariaLabel: 'Tilde', bytes: '~' },
  { id: 'slash', label: '/', ariaLabel: 'Slash', bytes: '/' },
  { id: 'dash', label: '-', ariaLabel: 'Dash', bytes: '-' },
  { id: PASTE_KEY_ID, label: 'Paste', ariaLabel: 'Paste' },
];

export function withControl(data: string): string {
  if (data.length !== 1) return data;
  const code = data.toUpperCase().charCodeAt(0);
  if (code >= 0x40 && code <= 0x5f) return String.fromCharCode(code - 0x40);
  if (data === ' ') return '\x00';
  if (data === '?') return '\x7f';
  return data;
}
