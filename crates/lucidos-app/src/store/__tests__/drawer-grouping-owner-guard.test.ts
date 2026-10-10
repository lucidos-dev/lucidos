/**
 * The drawer grouping and the selected ongoing group each have one owner, and the
 * drawer's transition follows the grouping. Two halves pin that:
 *
 *  1. `setDrawerGrouping` and `setSelectedOngoingGroup` are the only writers. Each
 *     signal is module-private and exported read-only, so `tsc` rejects a
 *     direct write. This scan catches the escapes `tsc` cannot: a cast back to
 *     writable, a second signal, or a write to the persisted key.
 *  2. The drawer's navigation cover keys on the grouping, through
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
  .filter(f => f.rel !== OWNER && f.rel !== 'store/__tests__/drawer-grouping-owner-guard.test.ts')
  .filter(f => !(allowTests && /\.test\.tsx?$/.test(f.rel)))
  .filter(f => pattern.test(f.text))
  .map(f => f.rel);

const OWNED = [
  { state: 'drawerGroupingState', reader: 'drawerGrouping', setter: 'setDrawerGrouping', type: 'DrawerGrouping', arg: 'grouping', key: 'lucidos-drawer-grouping' },
  { state: 'selectedOngoingGroupState', reader: 'selectedOngoingGroup', setter: 'setSelectedOngoingGroup', type: 'OngoingGroup', arg: 'group', key: 'lucidos-drawer-selected-ongoing-group' },
];
const esc = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

describe.each(OWNED)('$setter is the one writer of $reader', ({ state, reader, setter, type, arg, key }) => {
  const store = read(OWNER);

  it('keeps the writable signal private and exports it read-only', () => {
    expect(store).toMatch(new RegExp(`^const ${state} = signal<${esc(type)}>\\(`, 'm'));
    expect(store).toMatch(new RegExp(`^export const ${reader}: ReadonlySignal<${esc(type)}> = ${state};$`, 'm'));
    expect(store).not.toMatch(new RegExp(`export\\s*\\{[^}]*${state}`));
  });

  it('writes the signal in exactly one place, the setter', () => {
    const writes = store.match(new RegExp(`${state}\\.value\\s*=[^=]`, 'g')) ?? [];
    expect(writes).toHaveLength(1);
    const setter_ = store.slice(store.indexOf(`export function ${setter}`));
    expect(setter_.slice(0, setter_.indexOf('\n}'))).toContain(`${state}.value = ${arg};`);
  });

  it('has no writer anywhere else, cast or not', () => {
    expect(offenders(new RegExp(`${reader}(State)?(\\s+as\\s+[^;]+?\\))?\\s*\\.value\\s*=[^=]`))).toEqual([]);
    expect(offenders(new RegExp(`\\b${state}\\b`))).toEqual([]);
    expect(offenders(new RegExp(`${reader}\\s+as\\s+(unknown|Signal)`))).toEqual([]);
  });

  it('owns the persisted key: no other module writes it', () => {
    expect(offenders(new RegExp(`(setItem|removeItem)\\(\\s*['"]${esc(key)}['"]`), true)).toEqual([]);
  });
});

describe("the drawer's transition keys on the grouping", () => {
  const cover = read('components/drawer/ThreadFilterCover.tsx');

  it('keys the navigation cover through drawerSwapKey, with the live value', () => {
    expect(cover).toMatch(/const swapKey = drawerSwapKey\(open, drawerGrouping\.value\);/);
    expect(cover).toMatch(/<NavigationCover viewKey=\{swapKey\} motion="dip" \/>/);
    expect(cover).toMatch(/<LeavingViewDrawing swapKey=\{swapKey\} drawing=\{leaving\} \/>/);
    expect(cover).toMatch(/return filtersOpen \? 'filters' : `threads:\$\{grouping\}`;/);
  });

  it('takes the leaving drawing on every grouping change the drawer renders', () => {
    const drawer = read('components/drawer/ThreadDrawer.tsx');
    expect(drawer).toMatch(/<LeavingViewSnapshot grouping=\{grouping\} list=\{listRef\} drawing=\{leavingDrawing\}>/);
    expect(drawer).toMatch(/const grouping = drawerGrouping\.value;/);
  });
});
