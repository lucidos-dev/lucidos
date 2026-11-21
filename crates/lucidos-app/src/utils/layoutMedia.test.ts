import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, join, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import postcss from 'postcss';
import {
  DESKTOP_LAYOUT_QUERY,
  PHONE_LAYOUT_QUERY,
  expandLayoutMedia,
  isPhoneLayout,
} from './layoutMedia';
import { layoutMediaPlugin } from '../../vite/layoutMediaPlugin';

type Pointer = 'coarse' | 'fine' | 'none';
interface Viewport { width: number; height: number; pointer: Pointer }

/** Evaluates the subset of media query syntax the two layout queries use: a
 *  comma list of `and`-joined width, height and pointer features. */
function matches(query: string, v: Viewport): boolean {
  return query.split(/\s*,\s*/).some((part) => part.split(/\s+and\s+/).every((feature) => {
    const m = /^\((min|max)?-?(width|height|pointer):\s*(\w+?)(px)?\)$/.exec(feature.trim());
    if (!m) throw new Error(`unsupported feature: ${feature}`);
    const [, bound, name, value] = m;
    if (name === 'pointer') return v.pointer === value;
    const actual = name === 'width' ? v.width : v.height;
    return bound === 'min' ? actual >= Number(value) : actual <= Number(value);
  }));
}

const WIDTHS = [320, 390, 768, 769, 844, 932, 1024, 1280, 1920];
const HEIGHTS = [320, 390, 430, 500, 501, 700, 844, 1080];
const POINTERS: Pointer[] = ['coarse', 'fine', 'none'];
const GRID: Viewport[] = WIDTHS.flatMap((width) => HEIGHTS.flatMap((height) =>
  POINTERS.map((pointer) => ({ width, height, pointer }))));

describe('the phone layout and the desktop split', () => {
  it('cover every viewport exactly once', () => {
    for (const v of GRID) {
      expect([matches(PHONE_LAYOUT_QUERY, v), matches(DESKTOP_LAYOUT_QUERY, v)], JSON.stringify(v))
        .toEqual(expect.arrayContaining([true, false]));
    }
  });

  it('agree with what JS mounts', () => {
    for (const v of GRID) {
      const js = isPhoneLayout({ width: v.width, height: v.height, coarsePointer: v.pointer === 'coarse' });
      expect(js, JSON.stringify(v)).toBe(matches(PHONE_LAYOUT_QUERY, v));
    }
  });

  it('give a phone in landscape the phone layout, and nothing else that is wide', () => {
    expect(isPhoneLayout({ width: 844, height: 390, coarsePointer: true })).toBe(true);
    expect(isPhoneLayout({ width: 932, height: 430, coarsePointer: true })).toBe(true);
    // A tablet in landscape, a short desktop window, a touchscreen laptop.
    expect(isPhoneLayout({ width: 1180, height: 820, coarsePointer: true })).toBe(false);
    expect(isPhoneLayout({ width: 1000, height: 450, coarsePointer: false })).toBe(false);
    expect(isPhoneLayout({ width: 1366, height: 768, coarsePointer: false })).toBe(false);
  });
});

describe('expandLayoutMedia', () => {
  it('swaps a layout name for its query and leaves other params alone', () => {
    expect(expandLayoutMedia('(--phone-layout)')).toBe(PHONE_LAYOUT_QUERY);
    expect(expandLayoutMedia(' (--desktop-layout) ')).toBe(DESKTOP_LAYOUT_QUERY);
    expect(expandLayoutMedia('(max-width: 600px)')).toBe('(max-width: 600px)');
  });

  it('refuses a layout name combined with another feature', () => {
    expect(() => expandLayoutMedia('(--phone-layout) and (hover: none)')).toThrow(/stand alone/);
  });

  it('runs as the PostCSS plugin Vite loads', async () => {
    const out = await postcss([layoutMediaPlugin()]).process(
      '@media (--phone-layout) { .a { color: red; } }',
      { from: undefined },
    );
    expect(out.css).toContain(`@media ${PHONE_LAYOUT_QUERY} {`);
  });
});

describe('stylesheets name the layout instead of restating it', () => {
  const srcDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  // Served raw into app frames, outside Vite, so the names cannot expand there.
  const RAW_SERVED = 'styles/global/shared-components.css';

  function cssFiles(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry: { name: string; isDirectory(): boolean }) => {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) return cssFiles(path);
      return entry.name.endsWith('.css') ? [path] : [];
    });
  }

  it('writes no raw 768px layout query in a Vite-built stylesheet', () => {
    const offenders = cssFiles(srcDir)
      .filter((path) => relative(srcDir, path) !== RAW_SERVED)
      .filter((path) => /@media[^{]*\((max-width:\s*768px|min-width:\s*769px)\)/.test(readFileSync(path, 'utf8')))
      .map((path) => relative(srcDir, path));
    expect(offenders).toEqual([]);
  });

  it('uses no layout name in the stylesheet served raw', () => {
    expect(readFileSync(join(srcDir, RAW_SERVED), 'utf8')).not.toMatch(/--(phone|desktop)-layout/);
  });
});
