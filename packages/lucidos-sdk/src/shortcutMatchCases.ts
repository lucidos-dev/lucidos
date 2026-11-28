export interface ShortcutMatchCase {
  name: string;
  binding: { mod: boolean; shift: boolean; alt: boolean; key: string };
  event: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; key: string; code?: string };
  matches: boolean;
}

const MOD_SLASH = { mod: true, shift: false, alt: false, key: '/' };
const MOD_EQUALS = { mod: true, shift: false, alt: false, key: '=' };
const MOD_ZERO = { mod: true, shift: false, alt: false, key: '0' };
const MOD_SHIFT_ONE = { mod: true, shift: true, alt: false, key: '1' };
const MOD_K = { mod: true, shift: false, alt: false, key: 'k' };
const MOD_SHIFT_K = { mod: true, shift: true, alt: false, key: 'k' };

const cmd = (key: string, more: Partial<ShortcutMatchCase['event']> = {}) => ({
  metaKey: true, ctrlKey: false, shiftKey: false, altKey: false, key, ...more,
});

export const SHORTCUT_MATCH_CASES: readonly ShortcutMatchCase[] = [
  { name: 'US slash', binding: MOD_SLASH, event: cmd('/', { code: 'Slash' }), matches: true },
  { name: 'Norwegian slash is Shift+7', binding: MOD_SLASH, event: cmd('/', { shiftKey: true, code: 'Digit7' }), matches: true },
  { name: 'Norwegian equals is Shift+0', binding: MOD_EQUALS, event: cmd('=', { shiftKey: true, code: 'Digit0' }), matches: true },
  { name: 'US plus zooms in', binding: MOD_EQUALS, event: cmd('+', { shiftKey: true, code: 'Equal' }), matches: true },
  { name: 'Norwegian Shift+0 is not reset zoom', binding: MOD_ZERO, event: cmd('=', { shiftKey: true, code: 'Digit0' }), matches: false },
  { name: 'shifted digit reports its symbol', binding: MOD_SHIFT_ONE, event: cmd('!', { shiftKey: true, code: 'Digit1' }), matches: true },
  { name: 'AZERTY digit key reports a symbol', binding: MOD_ZERO, event: cmd('à', { code: 'Digit0' }), matches: true },
  { name: 'digit by key without code', binding: MOD_SHIFT_ONE, event: cmd('1', { shiftKey: true }), matches: true },
  { name: 'digit binding keeps Shift exact', binding: MOD_SHIFT_ONE, event: cmd('1', { code: 'Digit1' }), matches: false },
  { name: 'letter keeps Shift exact', binding: MOD_K, event: cmd('k', { shiftKey: true, code: 'KeyK' }), matches: false },
  { name: 'shifted letter binding', binding: MOD_SHIFT_K, event: cmd('K', { shiftKey: true, code: 'KeyK' }), matches: true },
  { name: 'punctuation still needs its modifiers', binding: MOD_SLASH, event: cmd('/', { metaKey: false, code: 'Slash' }), matches: false },
];
