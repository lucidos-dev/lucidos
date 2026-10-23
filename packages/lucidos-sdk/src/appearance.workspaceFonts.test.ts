/**
 * Workspace fonts in the appearance contract (ADR 0308).
 *
 * An entry reaches a surface from the engine, from a seed and from local
 * storage, and storage can be forged. So a client rebuilds every entry, and
 * these cases pin that nothing from the wire reaches CSS or a URL unchecked.
 */
import { describe, it, expect } from 'vitest';
import {
  FONT_STACKS,
  FONT_FEATURES_DEFAULT,
  isWorkspaceFontId,
  parseResolvedTheme,
  parseWorkspaceFont,
  resolveFont,
  resolveFontKey,
  sanitizeWorkspaceFont,
  sanitizeWorkspaceFonts,
  type WorkspaceFont,
} from './appearance';
import { WORKSPACE_FONT_FALLBACKS } from './generated/font-catalog';

/** An entry as `GET /api/v1/fonts` serves one. */
function served(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: 'ws-brand',
    label: 'Brand Sans',
    family: 'ws-brand',
    stack: "'ws-brand', system-ui, -apple-system, 'Segoe UI', sans-serif",
    kind: 'ui',
    group: 'sans',
    source: 'workspace',
    license: 'OFL-1.1',
    ligatures: false,
    faces: [{ path: 'fonts/brand/Brand.woff2', weight: '100 900', style: 'normal' }],
    ...overrides,
  };
}

function entry(overrides: Record<string, unknown> = {}): WorkspaceFont {
  const font = sanitizeWorkspaceFont(served(overrides));
  if (!font) throw new Error('the fixture should be valid');
  return font;
}

describe('workspace font ids', () => {
  it('are the prefix and a kebab slug, and nothing looser', () => {
    expect(isWorkspaceFontId('ws-brand')).toBe(true);
    expect(isWorkspaceFontId('ws-brand-sans-2')).toBe(true);
    for (const id of ['', 'brand', 'ws-', 'ws-Brand', 'ws--x', 'ws-x-', 'ws-a/b', 'fira-code']) {
      expect(isWorkspaceFontId(id)).toBe(false);
    }
    expect(isWorkspaceFontId(`ws-${'a'.repeat(40)}`)).toBe(true);
    expect(isWorkspaceFontId(`ws-${'a'.repeat(41)}`)).toBe(false);
  });
});

describe('sanitising a workspace font entry', () => {
  it('keeps a valid entry, with the stack rebuilt from its id and group', () => {
    const font = entry();
    expect(font.family).toBe('ws-brand');
    expect(font.stack).toBe(`'ws-brand', ${WORKSPACE_FONT_FALLBACKS.sans}`);
    expect(font.faces).toEqual([{ path: 'fonts/brand/Brand.woff2', weight: '100 900', style: 'normal' }]);
  });

  it('never trusts a stack or a family off the wire', () => {
    const font = entry({
      stack: "x; background: url(https://evil.example)",
      family: 'Comic Sans',
    });
    expect(font.stack).toBe(`'ws-brand', ${WORKSPACE_FONT_FALLBACKS.sans}`);
    expect(font.family).toBe('ws-brand');
  });

  it('refuses a face that points anywhere but a font file in its own directory', () => {
    for (const path of [
      'https://cdn.example.com/a.woff2',
      '//cdn.example.com/a.woff2',
      'fonts/other/a.woff2',
      'fonts/brand/../../secret.woff2',
      'fonts/brand/sub/a.woff2',
      'fonts/brand/a.html',
      'fonts/brand/a".woff2',
      'fonts/brand/.hidden.woff2',
      'artifacts/a.woff2',
    ]) {
      expect(sanitizeWorkspaceFont(served({ faces: [{ path, weight: '400', style: 'normal' }] })))
        .toBeNull();
    }
  });

  it('refuses a malformed id, group, weight, style or face list', () => {
    const face = { path: 'fonts/brand/a.woff2', weight: '400', style: 'normal' };
    for (const overrides of [
      { id: 'brand' },
      { id: 'toString' },
      { group: 'script' },
      { faces: [] },
      { faces: 'fonts/brand/a.woff2' },
      { faces: [{ ...face, weight: 'bold' }] },
      { faces: [{ ...face, weight: '0' }] },
      { faces: [{ ...face, weight: '900 100' }] },
      { faces: [{ ...face, style: 'oblique' }] },
      { faces: Array.from({ length: 17 }, () => face) },
    ]) {
      expect(sanitizeWorkspaceFont(served(overrides))).toBeNull();
    }
    expect(sanitizeWorkspaceFont(null)).toBeNull();
    expect(sanitizeWorkspaceFont([served()])).toBeNull();
  });

  it('falls back to the id for a missing label, and caps a long one', () => {
    expect(entry({ label: '' }).label).toBe('ws-brand');
    expect(entry({ label: 'x'.repeat(200) }).label).toHaveLength(60);
  });

  it('parses a stored entry, and anything corrupt to null', () => {
    expect(parseWorkspaceFont(JSON.stringify(served()))?.id).toBe('ws-brand');
    for (const raw of [null, '', '{', '"ws-brand"']) expect(parseWorkspaceFont(raw)).toBeNull();
  });

  it('drops the bad entries of a list and keeps the rest once each', () => {
    const list = sanitizeWorkspaceFonts([served(), served({ id: 'nope' }), served()]);
    expect(list.map(f => f.id)).toEqual(['ws-brand']);
    expect(sanitizeWorkspaceFonts('not a list')).toEqual([]);
  });
});

describe('resolving a workspace font', () => {
  const brand = entry();
  const code = entry({
    id: 'ws-code',
    group: 'mono',
    ligatures: true,
    faces: [{ path: 'fonts/code/a.woff2', weight: '400', style: 'normal' }],
  });

  it('paints a picked workspace font whose entry is at hand', () => {
    const font = resolveFont('ws-brand', {}, [brand]);
    expect(font.key).toBe('ws-brand');
    expect(font.stack).toBe(brand.stack);
    expect(font.features).toEqual(FONT_FEATURES_DEFAULT);
    expect(font.workspaceFont).toBe(brand);
  });

  it('falls back, as for any unknown font, when the entry is missing', () => {
    expect(resolveFontKey('ws-gone', { ui: 'geist' }, [brand])).toBe('geist');
    const font = resolveFont('ws-gone', {}, [brand]);
    expect(font.key).toBe('fira-code');
    expect(font.stack).toBe(FONT_STACKS['fira-code']);
    expect(font.workspaceFont).toBeNull();
  });

  it("follows a theme's workspace font only when its entry is at hand", () => {
    expect(resolveFontKey('theme', { ui: 'ws-brand' }, [brand])).toBe('ws-brand');
    expect(resolveFontKey('theme', { ui: 'ws-brand' }, [])).toBe('fira-code');
  });

  it("reads a workspace font's ligatures from its entry", () => {
    const font = resolveFont('ws-code', {}, [code]);
    expect(font.features.code).toBe('"liga" 1, "calt" 1');
    expect(font.features.text).toBe('"liga" 0, "calt" 0');
  });

  it("carries a theme's workspace fonts through parsing, and drops bad ones", () => {
    const theme = parseResolvedTheme(JSON.stringify({
      dark: {},
      light: {},
      fonts: { ui: 'ws-brand', mono: 'ws-bad/id' },
      workspace_fonts: [served(), served({ id: 'nope' })],
    }));
    expect(theme.fonts).toEqual({ ui: 'ws-brand' });
    expect(theme.workspace_fonts.map(f => f.id)).toEqual(['ws-brand']);
  });
});
