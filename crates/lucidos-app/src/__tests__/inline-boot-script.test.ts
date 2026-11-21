import { describe, it, expect } from 'vitest';
import { inlineBootScript } from '../../vite/inlineBootScript';

const MARKER = '<!-- boot -->';
const PAGE = `<html><head>${MARKER}<title>x</title></head></html>`;

describe('inlineBootScript', () => {
  it('inserts the bundle verbatim, dollar patterns included', () => {
    // Each of these is a replacement-string pattern, and a template literal
    // ending in a regex anchor spells the second one.
    const bundle = 'var re = new RegExp(`^${a}${b}$`); var s = "$& $\' $$";';
    expect(inlineBootScript(PAGE, MARKER, bundle)).toBe(
      `<html><head><script>\n${bundle}</script><title>x</title></head></html>`,
    );
  });

  it('refuses a bundle that would close its own tag', () => {
    expect(() => inlineBootScript(PAGE, MARKER, 'var s = "</script>";')).toThrow(/<\/script/);
  });
});
