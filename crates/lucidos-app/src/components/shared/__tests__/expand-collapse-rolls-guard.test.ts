import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

/**
 * Every expand and collapse animates (`.claude/rules/frontend.md` § Every
 * Expand and Collapse Rolls). An inline toggle says so with `aria-expanded`,
 * and its content rolls through `<Disclosure>`. A popover toggle also carries
 * `aria-expanded`, beside an `aria-haspopup`, and opens through `<Overlay>`
 * instead, so it is not counted here. A pressed toggle, such as the header's
 * search button, carries no `aria-expanded`, so this scan reads the content
 * side too.
 *
 * A source scan, like `skeleton-guard`: jsdom runs no animation to assert.
 */

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../../..'); // crates/lucidos-app/src

/** Files whose inline toggle rolls somewhere other than a `<Disclosure>` in
 *  the same file, and where. */
const ROLLS_ELSEWHERE: Record<string, string> = {
  'components/drawer/ThreadDrawer.tsx': 'the drawer rolls its rows as FLIP copies (useFlipAnimation)',
  'components/shared/SectionHeader.tsx': 'each caller rolls its section body in a <Disclosure>',
  'components/layout/ThreadFilterButton.tsx': 'it swaps the drawer view under the navigation cover (ThreadFilterCover.tsx)',
};

/** How far from `aria-expanded` an `aria-haspopup` still belongs to the same
 *  element: a JSX opening tag with one attribute per line. */
const SAME_ELEMENT_LINES = 12;

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== '__tests__') out.push(...sourceFiles(full));
    } else if (entry.name.endsWith('.tsx') && !entry.name.endsWith('.test.tsx')) {
      out.push(full);
    }
  }
  return out;
}

/** Whether the file holds an inline toggle: an `aria-expanded` with no
 *  `aria-haspopup` on its element. */
function hasInlineToggle(src: string): boolean {
  const lines = src.split('\n');
  return lines.some((line, i) => {
    if (!line.includes('aria-expanded')) return false;
    const element = lines.slice(Math.max(0, i - SAME_ELEMENT_LINES), i + SAME_ELEMENT_LINES + 1).join('\n');
    return !element.includes('aria-haspopup');
  });
}

/** A mount gated on an open or expanded flag, as `{xOpen.value && <X />}` or
 *  `xOpen.value ? <X /> : null`. It appears and vanishes in one frame, whoever
 *  flips the flag. The toggle can be anywhere, even a header button in
 *  another file, so this check reads the mount and not the toggle. A ternary
 *  whose else branch is JSX swaps two drawings, an icon say, and is not one. */
const GATED_MOUNT = new RegExp(
  String.raw`(?<![!\w])\w*(?:[Oo]pen|[Ee]xpanded)(?:\.value)?\s*` +
    String.raw`(?:&&\s*\(?\s*<|\?\s*\(?\s*<(?:(?!\s:\s*[<(])[\s\S]){0,600}?\s:\s*null\b)`,
);

const files = sourceFiles(resolve(SRC, 'components')).map((full) => ({
  path: relative(SRC, full),
  src: readFileSync(full, 'utf8'),
}));

describe('every expand and collapse rolls', () => {
  it('rolls each inline toggle through <Disclosure>', () => {
    const unrolled = files
      .filter(({ path, src }) => hasInlineToggle(src) && !src.includes('<Disclosure') && !(path in ROLLS_ELSEWHERE))
      .map(({ path }) => path);
    expect(unrolled).toEqual([]);
  });

  it('rolls each mount gated on an open flag, or opens it as an <Overlay>', () => {
    const bare = files
      .filter(({ src }) => GATED_MOUNT.test(src) && !src.includes('<Disclosure') && !src.includes('<Overlay'))
      .map(({ path }) => path);
    expect(bare).toEqual([]);
  });

  it('reads a gated mount in both of its shapes', () => {
    expect(GATED_MOUNT.test('{searchOpen.value && <SearchBar />}')).toBe(true);
    expect(GATED_MOUNT.test('{expanded && (\n  <Rows />\n)}')).toBe(true);
    expect(GATED_MOUNT.test('return cameraOpen.value ? <Camera /> : null;')).toBe(true);
    expect(GATED_MOUNT.test('{open ? <Panel onClose={() => close()} /> : null}')).toBe(true);
    expect(GATED_MOUNT.test('{open ? (\n  <div class="a">hi</div>\n) : null}')).toBe(true);
  });

  it('leaves a drawing swap and a negated flag alone', () => {
    expect(GATED_MOUNT.test('{open ? <ChevronDownIcon /> : <ChevronRightIcon />}')).toBe(false);
    expect(GATED_MOUNT.test('{open ? (\n  <Down />\n) : (\n  <Right />\n)}')).toBe(false);
    expect(GATED_MOUNT.test('{!searchOpen.value && <Hint />}')).toBe(false);
  });

  it('keeps no stale exception', () => {
    const stale = Object.keys(ROLLS_ELSEWHERE).filter((path) => {
      const file = files.find((f) => f.path === path);
      return !file || !hasInlineToggle(file.src);
    });
    expect(stale).toEqual([]);
  });

  it('counts the side question, whose card folds to a row', () => {
    const card = files.find((f) => f.path === 'components/chat/SideQuestionCard.tsx')!;
    expect(hasInlineToggle(card.src)).toBe(true);
    expect(card.src).toContain('<Disclosure');
  });
});
