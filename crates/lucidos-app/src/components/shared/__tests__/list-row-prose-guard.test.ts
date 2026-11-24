import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';


const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../../..');

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) out.push(...sourceFiles(full));
    else if (entry.name.endsWith('.tsx') && !/\.test\.tsx$/.test(entry.name)) out.push(full);
  }
  return out;
}

const BASE_CLASS = /list-row-details(?!-prose)/g;
const INLINE_ELEMENT = /<(?:code|strong|em)[\s>]/;

function bodyEnd(src: string, from: number): number {
  const ends = ['</div>', '</span>', '</SkText>'].map((t) => src.indexOf(t, from)).filter((i) => i >= 0);
  return ends.length ? Math.min(...ends) : src.length;
}

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
