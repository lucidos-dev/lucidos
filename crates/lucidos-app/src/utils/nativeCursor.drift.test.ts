/**
 * The native cursor table cannot fall behind our CSS.
 *
 * `utils/nativeCursor.ts` forwards whatever keyword the browser computed, and
 * `src/cursor.rs` turns it into a `tao::CursorIcon`. A keyword missing from
 * that table falls back to the arrow. That is the exact bug the mirroring
 * exists to fix, and it fails in silence.
 *
 * The table is total over the CSS keyword set, so this should never fire. That
 * is the point: it proves the claim against the source instead of trusting it.
 * The Rust file is read as text, the same way `sse-event-coverage.test.ts`
 * reads `RESERVED_TYPE_NAMES`.
 *
 * Host sources only. An app iframe's cursors never reach the host document, so
 * the engine-served `sdk_iframe.css` is deliberately out of scope.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';
import { cursorKeyword } from './nativeCursor';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '..');

/** The keywords `src/cursor.rs` maps, read out of the table's own literals. */
const mapped = new Set(
  [...readFileSync(resolve(SRC, 'cursor.rs'), 'utf8')
    .matchAll(/\("([a-z-]+)",\s*CursorIcon::/g)].map((m) => m[1]),
);

function sourceFiles(dir: string, exts: string[]): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) out.push(...sourceFiles(full, exts));
    else if (exts.some((e) => entry.name.endsWith(e))) out.push(full);
  }
  return out;
}

/** A value that cannot reach the wire as itself: the CSS-wide keywords all
 *  resolve to something else before `getComputedStyle` answers, and a value
 *  built from a variable is not readable here. */
function unreadable(value: string): boolean {
  return /^(inherit|initial|unset|revert|revert-layer)$/.test(value)
    || value.includes('var(') || value.includes('${');
}

/** Every cursor keyword the host's own source can put on the wire, against the
 *  file it was found in. */
function declaredCursors(): { keyword: string; where: string }[] {
  const found: { keyword: string; where: string }[] = [];
  const add = (raw: string, file: string) => {
    const value = raw.trim();
    if (unreadable(value)) return;
    found.push({ keyword: cursorKeyword(value), where: relative(SRC, file) });
  };

  for (const file of sourceFiles(resolve(SRC, 'styles'), ['.css'])) {
    // Comments stripped first: this pattern needs no quotes, so prose about a
    // cursor would otherwise read as a declaration.
    const css = readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    for (const m of css.matchAll(/(?:^|[;{\s])cursor:\s*([^;{}]+)/g)) add(m[1], file);
  }

  // `style={{ cursor: 'x' }}` in TSX, and `el.style.cursor = 'x'` anywhere.
  // The quotes are what keep prose out, so comments stay in.
  for (const file of sourceFiles(SRC, ['.ts', '.tsx'])) {
    if (/\.test\.tsx?$/.test(file)) continue;
    const src = readFileSync(file, 'utf8');
    for (const m of src.matchAll(/cursor\s*[:=]\s*['"]([^'"]*)['"]/g)) add(m[1], file);
  }
  return found;
}

describe('native cursor table', () => {
  it('parses, and covers the whole CSS keyword set', () => {
    // 36 is what the `cursor` property accepts. A broken parse above would show
    // up here as a short table rather than as a pass.
    expect(mapped.size).toBe(36);
  });

  it('maps the two keywords that ride the wire without being declared', () => {
    // `auto` is the initial value, so most of the app computes it. `default` is
    // what the reconciler hands back when the pointer leaves the document.
    expect(mapped.has('auto')).toBe(true);
    expect(mapped.has('default')).toBe(true);
  });

  it('maps every cursor the host source declares', () => {
    const declared = declaredCursors();
    // A scan that found nothing would pass the check below while proving
    // nothing, so the sweep itself has to have swept something.
    expect(declared.length).toBeGreaterThan(20);
    const missing = declared.filter(({ keyword }) => !mapped.has(keyword));
    expect(missing).toEqual([]);
  });
});
