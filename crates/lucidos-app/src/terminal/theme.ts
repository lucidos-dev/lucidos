import type { ITheme } from '@xterm/xterm';
import { resolvedTokenHex } from '../utils/cssColor';

type TokenName = `--${string}`;

const THEME_TOKENS = {
  background: '--bg-primary',
  foreground: '--text-primary',
  cursor: '--text-primary',
  cursorAccent: '--bg-primary',
  selectionBackground: '--accent',
  selectionForeground: '--text-on-accent',
  black: '--bg-tertiary',
  red: '--accent-red',
  green: '--accent-green',
  yellow: '--accent-yellow',
  blue: '--syntax-keyword',
  magenta: '--syntax-control',
  cyan: '--syntax-type',
  white: '--text-secondary',
  brightBlack: '--text-muted',
  brightRed: '--accent-orange',
  brightGreen: '--syntax-number',
  brightYellow: '--syntax-function',
  brightBlue: '--accent-light',
  brightMagenta: '--syntax-string',
  brightCyan: '--syntax-key',
  brightWhite: '--text-strong',
} as const satisfies Partial<Record<keyof ITheme, TokenName>>;

export function terminalTheme(resolve: (token: TokenName) => string | null = resolvedTokenHex): ITheme {
  const theme: ITheme = {};
  for (const [slot, token] of Object.entries(THEME_TOKENS) as [keyof typeof THEME_TOKENS, TokenName][]) {
    const color = resolve(token);
    if (color) theme[slot] = color;
  }
  return theme;
}
