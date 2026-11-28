import { describe, it, expect } from 'vitest';
import { terminalTheme } from './theme';

describe('the terminal theme', () => {
  it('reads every colour from a theme token', () => {
    const seen: string[] = [];
    const theme = terminalTheme((token) => { seen.push(token); return `c(${token})`; });
    expect(theme.background).toBe('c(--bg-primary)');
    expect(theme.foreground).toBe('c(--text-primary)');
    expect(theme.red).toBe('c(--accent-red)');
    expect(seen.every((token) => token.startsWith('--'))).toBe(true);
  });

  it('leaves a slot to xterm when its token does not resolve', () => {
    const theme = terminalTheme((token) => (token === '--bg-primary' ? '#000000' : null));
    expect(theme).toEqual({ background: '#000000', cursorAccent: '#000000' });
  });
});
