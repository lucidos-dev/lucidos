import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

/**
 * `.list-row-details` is a FLEX row whose 0.75rem gap is the separator between
 * metadata fields (see `.claude/rules/frontend.md` § List rows). Flex blockifies
 * every inline child, so a *sentence* placed in that slot loses normal inline
 * wrapping: each `<strong>`/`<code>` becomes its own flex item, the gap opens
 * holes mid-sentence, and the punctuation that follows the element is stranded
 * at the start of the next line (the Mobile Access "weird dots", 2026-08-04).
 *
 * Prose belongs in `class="list-row-details list-row-details-prose"`, which
 * restores block flow. There is no jsdom in the test infra and no CSS applied
 * even if there were, so the layout itself cannot be asserted: this is a
 * source-scan, matching the `skeleton-guard` / `no-raw-storage` precedent.
 */

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../../..'); // crates/lucidos-app/src

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) out.push(...sourceFiles(full));
    else if (entry.name.endsWith('.tsx') && !/\.test\.tsx$/.test(entry.name)) out.push(full);
  }
  return out;
}

// The lookahead skips the modifier's own occurrences; the base class still
// matches inside `class="list-row-details list-row-details-prose"` (a space
// follows it there), and that pair is filtered out by the opening-tag check.
const BASE_CLASS = /list-row-details(?!-prose)/g;
const INLINE_ELEMENT = /<(?:code|strong|em)[\s>]/;

/** Where the element opened at `start` ends, as far as this scan cares. A
 *  details slot never nests a block, so the first close of either kind ends it;
 *  stopping early can only under-report, never flag a well-formed row. */
function bodyEnd(src: string, from: number): number {
  const ends = ['</div>', '</span>'].map((t) => src.indexOf(t, from)).filter((i) => i >= 0);
  return ends.length ? Math.min(...ends) : src.length;
}

/** Is the occurrence at `start` an actual class attribute, rather than the name
 *  appearing in a comment? An open `class=` with no `>` since means we are still
 *  inside the opening tag, which covers both the quoted and template-literal
 *  forms. */
function inClassAttribute(src: string, start: number): boolean {
  const before = src.slice(Math.max(0, start - 60), start);
  const attr = before.lastIndexOf('class=');
  return attr >= 0 && !before.slice(attr).includes('>');
}

describe('list-row-details prose guard', () => {
  it('a details slot containing inline markup declares itself prose', () => {
    const offenders: string[] = [];
    for (const file of sourceFiles(SRC)) {
      const src = readFileSync(file, 'utf8');
      for (const match of src.matchAll(BASE_CLASS)) {
        const start = match.index as number;
        if (!inClassAttribute(src, start)) continue;
        const tagEnd = src.indexOf('>', start);
        if (tagEnd < 0) continue;
        // The modifier may sit anywhere in the same class attribute.
        if (src.slice(start, tagEnd).includes('list-row-details-prose')) continue;
        if (!INLINE_ELEMENT.test(src.slice(tagEnd + 1, bodyEnd(src, tagEnd + 1)))) continue;
        const line = src.slice(0, start).split('\n').length;
        offenders.push(`${relative(SRC, file)}:${line}`);
      }
    }
    expect(
      offenders,
      'a sentence with inline <code>/<strong>/<em> needs class="list-row-details list-row-details-prose". The bare flex class strands its punctuation on the next line.',
    ).toEqual([]);
  });
});
