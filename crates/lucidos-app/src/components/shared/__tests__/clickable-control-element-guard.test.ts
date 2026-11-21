/**
 * An add card and a Settings home row are BUTTONS, not clickable divs.
 *
 * A `<div onClick=…>` takes no tab stop, reports no role to assistive tech, and
 * ignores Enter and Space, so a control built that way is pointer-only. All
 * seven add cards were divs until 2026-08-10: a keyboard user could tab through
 * the Remove button of every repository in Settings and never reach the one
 * control that adds one. The Settings home rows were the same, which put every
 * settings category (and so every control inside one) out of keyboard reach.
 *
 * A native `<button>` is what fixes it, rather than `role="button"` +
 * `tabIndex={0}` + a hand-written `onKeyDown`: the element brings the tab stop,
 * the role, Enter AND Space, and `:focus-visible` with nothing to get wrong.
 * (The two `role="button"` sites in components/picker/ are the shape this
 * avoids: both handle Enter and silently drop Space.)
 *
 * There is no jsdom in the test infra and no CSS applied even if there were, so
 * this is a source scan, matching the `list-row-prose-guard` / `skeleton-guard`
 * precedent. It is what stops the eighth add card being written as a div again.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../../..'); // crates/lucidos-app/src

/** Classes that may only ever be applied to a `<button>`. */
const BUTTON_ONLY = ['list-row-add-card', 'settings-nav-row'];

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) out.push(...sourceFiles(full));
    else if (entry.name.endsWith('.tsx') && !/\.test\.tsx$/.test(entry.name)) out.push(full);
  }
  return out;
}

/** Is the occurrence at `start` inside a class attribute, rather than in a
 *  comment? An open `class=` with no `>` since means we are still inside the
 *  opening tag, which covers the quoted and template-literal forms alike.
 *  (Same check as list-row-prose-guard, which documents the reasoning.) */
function inClassAttribute(src: string, start: number): boolean {
  const before = src.slice(Math.max(0, start - 80), start);
  const attr = before.lastIndexOf('class=');
  return attr >= 0 && !before.slice(attr).includes('>');
}

/** Name of the element whose opening tag encloses `pos`. Scans back to the
 *  nearest `<` that actually starts a tag name, so a `<` inside an earlier
 *  attribute expression is skipped rather than read as the tag. */
function openingTagName(src: string, pos: number): string | null {
  for (let i = pos; i >= 0; i--) {
    if (src[i] !== '<') continue;
    const m = /^<([A-Za-z][\w.]*)/.exec(src.slice(i, i + 40));
    if (m) return m[1];
  }
  return null;
}

describe('clickable-control element guard', () => {
  for (const className of BUTTON_ONLY) {
    it(`.${className} is only ever applied to a <button>`, () => {
      const offenders: string[] = [];
      for (const file of sourceFiles(SRC)) {
        const src = readFileSync(file, 'utf8');
        for (const match of src.matchAll(new RegExp(className, 'g'))) {
          const start = match.index as number;
          if (!inClassAttribute(src, start)) continue;
          const tag = openingTagName(src, start);
          if (tag === 'button') continue;
          const line = src.slice(0, start).split('\n').length;
          offenders.push(`${relative(SRC, file)}:${line} <${tag ?? '?'}>`);
        }
      }
      expect(
        offenders,
        `.${className} is a control: it must be a <button> so it takes a tab stop and answers Enter and Space. For an add card, render <ListRowAddCard label=… onClick=… /> instead of hand-writing the markup.`,
      ).toEqual([]);
    });
  }

  it('actually reaches the markup, so a rename cannot make the scan vacuous', () => {
    // Without this, renaming either class would leave both scans above passing
    // on an empty set and the regression would walk straight back in.
    const seen = new Map(BUTTON_ONLY.map(c => [c, 0]));
    for (const file of sourceFiles(SRC)) {
      const src = readFileSync(file, 'utf8');
      for (const className of BUTTON_ONLY) {
        for (const match of src.matchAll(new RegExp(className, 'g'))) {
          if (inClassAttribute(src, match.index as number)) {
            seen.set(className, (seen.get(className) as number) + 1);
          }
        }
      }
    }
    for (const [className, count] of seen) {
      expect(count, `.${className} is applied nowhere: was it renamed?`).toBeGreaterThan(0);
    }
  });
});
