import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const COMPONENTS_ROOT: string = resolve(here, '..');

function walkTsx(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = resolve(dir, entry);
    if (statSync(path).isDirectory()) out.push(...walkTsx(path));
    else if (path.endsWith('.tsx')) out.push(path);
  }
  return out;
}

function fieldTags(source: string): string[] {
  const tags: string[] = [];
  const opener = /<input\b/g;
  let match: RegExpExecArray | null;
  while ((match = opener.exec(source)) !== null) {
    const end = source.indexOf('/>', match.index);
    if (end < 0) break;
    tags.push(source.slice(match.index, end));
  }
  return tags;
}

function commitsOnBlurAndCancelsOnEscape(tag: string): boolean {
  const blur = /\bonBlur=\{([^}]*)\}/.exec(tag);
  return blur !== null && /commit|save/i.test(blur[1]) && tag.includes("'Escape'");
}

describe('Escape on a blur-commit field', () => {
  it('finds the fields it guards', () => {
    const guarded = walkTsx(COMPONENTS_ROOT).flatMap((file) =>
      fieldTags(readFileSync(file, 'utf-8')).filter(commitsOnBlurAndCancelsOnEscape),
    );
    expect(guarded.length).toBeGreaterThanOrEqual(5);
  });

  it('leaves the key to the field, so the blur cannot commit first', () => {
    const offenders = walkTsx(COMPONENTS_ROOT).flatMap((file) =>
      fieldTags(readFileSync(file, 'utf-8'))
        .filter(commitsOnBlurAndCancelsOnEscape)
        .filter((tag) => !tag.includes('data-escape-self'))
        .map(() => relative(COMPONENTS_ROOT, file)),
    );
    expect(offenders).toEqual([]);
  });

  it('spends the Escape, so an open inline form stays open', () => {
    const offenders = walkTsx(COMPONENTS_ROOT).flatMap((file) =>
      fieldTags(readFileSync(file, 'utf-8'))
        .filter(commitsOnBlurAndCancelsOnEscape)
        .filter((tag) => !/'Escape'\)\s*\{\s*e\.preventDefault\(\)/.test(tag))
        .map(() => relative(COMPONENTS_ROOT, file)),
    );
    expect(offenders).toEqual([]);
  });
});
