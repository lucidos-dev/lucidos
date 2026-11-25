import { describe, it, expect, beforeEach, vi } from 'vitest';

const api = vi.hoisted(() => ({
  listFonts: vi.fn(),
  listFontFiles: vi.fn(),
  writeFontFile: vi.fn(),
  deleteFontFile: vi.fn(),
}));
vi.mock('../../api/client', () => api);

import {
  guessFace, installWorkspaceFont, removeWorkspaceFont, safeFileName, slugFromLabel,
} from './workspaceFonts';

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

describe('installWorkspaceFont', () => {
  const install = {
    label: 'Brand Sans',
    group: 'sans' as const,
    ligatures: false,
    files: [{ file: new File(['x'], 'Brand.woff2'), weight: '400', style: 'normal' as const }],
  };

  beforeEach(() => {
    api.listFonts.mockReset().mockResolvedValue({ fonts: [], invalid: [] });
    api.listFontFiles.mockReset().mockResolvedValue([]);
    api.writeFontFile.mockReset().mockResolvedValue(undefined);
    api.deleteFontFile.mockReset().mockResolvedValue(undefined);
  });

  it('picks a directory a broken font already holds, so its writes cannot overwrite that font', async () => {
    api.listFonts.mockResolvedValue({ fonts: [], invalid: [{ id: 'ws-brand-sans', reason: 'no manifest' }] });

    const id = await installWorkspaceFont(install);

    expect(id).toBe('ws-brand-sans-2');
    expect(api.writeFontFile.mock.calls.map((c) => c[0])).toContain('fonts/brand-sans-2/font.json');
  });
});

describe('removeWorkspaceFont', () => {
  beforeEach(() => {
    api.listFonts.mockReset().mockResolvedValue({ fonts: [], invalid: [] });
    api.listFontFiles.mockReset();
    api.deleteFontFile.mockReset().mockResolvedValue(undefined);
  });

  it('deletes only the broken font named, even when its raw name holds regex characters', async () => {
    api.listFontFiles.mockResolvedValue([
      'fonts/my.font/font.json',
      'fonts/my.font/a.woff2',
      'fonts/myxfont/font.json',
      'fonts/myxfont/b.woff2',
    ]);

    await removeWorkspaceFont('ws-my.font');

    expect(api.deleteFontFile.mock.calls.map((c) => c[0]).sort())
      .toEqual(['fonts/my.font/a.woff2', 'fonts/my.font/font.json']);
  });
});
