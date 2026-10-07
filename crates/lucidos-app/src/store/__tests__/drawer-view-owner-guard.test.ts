/**
 * The status filter has one owner, and its transition follows the value. Two
 * halves pin that:
 *
 *  1. `setDrawerView` is the only writer. The signal is module-private, and
 *     `drawerView` is exported read-only, so `tsc` rejects a direct write. This
 *     scan catches the escapes `tsc` cannot: a cast back to writable, a second
 *     signal, or a write to the persisted key.
 *  2. The drawer's navigation cover keys on the value, through
 *     `drawerSwapKey`. So no caller of the owner can skip the dip.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..');
const read = (rel: string): string => readFileSync(resolve(SRC, rel), 'utf8');

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const abs = resolve(dir, entry.name);
    if (entry.isDirectory()) out.push(...sourceFiles(abs));
    else if (/\.tsx?$/.test(entry.name)) out.push(abs);
  }
  return out;
}

const OWNER = 'store/store.ts';
const files = sourceFiles(SRC).map((abs: string) => ({ rel: relative(SRC, abs) as string, text: readFileSync(abs, 'utf8') as string }));
const offenders = (pattern: RegExp, allowTests = false) => files
  .filter(f => f.rel !== OWNER && f.rel !== 'store/__tests__/drawer-view-owner-guard.test.ts')
  .filter(f => !(allowTests && /\.test\.tsx?$/.test(f.rel)))
  .filter(f => pattern.test(f.text))
  .map(f => f.rel);

describe('setDrawerView is the one writer of the status filter', () => {
  const store = read(OWNER);

  it('keeps the writable signal private and exports it read-only', () => {
    expect(store).toMatch(/^const drawerViewState = signal<DrawerView>\(/m);
    expect(store).toMatch(/^export const drawerView: ReadonlySignal<DrawerView> = drawerViewState;$/m);
    expect(store).not.toMatch(/export\s*\{[^}]*drawerViewState/);
  });

  it('writes the signal in exactly one place, the setter', () => {
    const writes = store.match(/drawerViewState\.value\s*=[^=]/g) ?? [];
    expect(writes).toHaveLength(1);
    const setter = store.slice(store.indexOf('export function setDrawerView'));
    expect(setter.slice(0, setter.indexOf('\n}'))).toMatch(/drawerViewState\.value = view;/);
  });

  it('has no writer anywhere else, cast or not', () => {
    expect(offenders(/drawerView(State)?(\s+as\s+[^;]+?\))?\s*\.value\s*=[^=]/)).toEqual([]);
    expect(offenders(/\bdrawerViewState\b/)).toEqual([]);
    expect(offenders(/drawerView\s+as\s+(unknown|Signal)/)).toEqual([]);
  });

  it('owns the persisted key: no other module writes it', () => {
    expect(offenders(/(setItem|removeItem)\(\s*['"]lucidos-alt-view['"]/, true)).toEqual([]);
  });
});

describe("the drawer's transition keys on the status filter", () => {
  const cover = read('components/drawer/ThreadFilterCover.tsx');

  it('keys the navigation cover through drawerSwapKey, with the live value', () => {
    expect(cover).toMatch(/const swapKey = drawerSwapKey\(open, drawerView\.value\);/);
    expect(cover).toMatch(/<NavigationCover viewKey=\{swapKey\} motion="dip" \/>/);
    expect(cover).toMatch(/<LeavingViewDrawing swapKey=\{swapKey\} drawing=\{leaving\} \/>/);
    expect(cover).toMatch(/return filtersOpen \? 'filters' : `threads:\$\{view\}`;/);
  });

  it('takes the leaving drawing on every view change the drawer renders', () => {
    const drawer = read('components/drawer/ThreadDrawer.tsx');
    expect(drawer).toMatch(/<LeavingViewSnapshot view=\{view\} list=\{listRef\} drawing=\{leavingDrawing\}>/);
    expect(drawer).toMatch(/const view = drawerView\.value;/);
  });
});
