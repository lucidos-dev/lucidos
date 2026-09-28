import { describe, it, expect } from 'vitest';
import { guessFace, safeFileName, slugFromLabel } from './workspaceFonts';

describe('slugFromLabel', () => {
  it('makes a directory name the engine accepts', () => {
    expect(slugFromLabel('Brand Sans')).toBe('brand-sans');
    expect(slugFromLabel('  Söhne Mono 2 ')).toBe('s-hne-mono-2');
    expect(slugFromLabel('---')).toBe('');
    expect(slugFromLabel('x'.repeat(60))).toHaveLength(40);
    expect(slugFromLabel(`${'a'.repeat(39)} b`)).toBe('a'.repeat(39));
  });
});

describe('safeFileName', () => {
  it('keeps a plain name and lowercases the extension', () => {
    expect(safeFileName('BrandSans-Bold.WOFF2', new Set())).toBe('BrandSans-Bold.woff2');
  });

  it('replaces anything a URL or a path could read as structure', () => {
    expect(safeFileName('../My Font (1).ttf', new Set())).toBe('My-Font-1-.ttf');
    expect(safeFileName('.hidden.otf', new Set())).toBe('hidden.otf');
  });

  it('makes each name unique within one font', () => {
    const taken = new Set<string>();
    expect(safeFileName('a.woff2', taken)).toBe('a.woff2');
    expect(safeFileName('a.woff2', taken)).toBe('a-2.woff2');
  });
});

describe('guessFace', () => {
  it('reads the weight and style a file name spells', () => {
    expect(guessFace('Brand-Regular.woff2')).toEqual({ weight: '400', style: 'normal' });
    expect(guessFace('Brand-Bold.woff2')).toEqual({ weight: '700', style: 'normal' });
    expect(guessFace('Brand-ExtraBold.woff2')).toEqual({ weight: '800', style: 'normal' });
    expect(guessFace('Brand-SemiBoldItalic.ttf')).toEqual({ weight: '600', style: 'italic' });
    expect(guessFace('Brand-ExtraLight.otf').weight).toBe('200');
    expect(guessFace('Brand-Light.otf').weight).toBe('300');
    expect(guessFace('Brand[wght].woff2').weight).toBe('100 900');
    expect(guessFace('Brand-VF.woff2').weight).toBe('100 900');
  });
});
