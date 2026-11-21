// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, join, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

/** A change's `description` is its commit subjects, newest first. Printed raw,
 *  a toast or a Changes row names the change by its latest fix, or by every
 *  commit it holds. Every surface naming a change goes through
 *  `changeHeadline` (store/changeHeadline.ts) instead. */
const SRC: string = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

function sources(dir: string): [string, string][] {
  const names: string[] = readdirSync(join(SRC, dir));
  return names
    .filter(name => /\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name))
    .map((name): [string, string] => [`${dir}/${name}`, readFileSync(join(SRC, dir, name), 'utf8')]);
}

describe('surfaces that name a change', () => {
  it('no change toast passes a raw description', () => {
    const offenders = sources('store/actions').flatMap(([path, body]) =>
      body
        .split('\n')
        .filter(line => line.includes('changeToastMessage(') && line.includes('.description'))
        .map(line => `${path}: ${line.trim()}`));
    expect(offenders).toEqual([]);
  });

  it('the Changes panel names a change by its headline', () => {
    const offenders = sources('components/changes').filter(([, body]) => /\bchange!?\.description\b/.test(body));
    expect(offenders.map(([path]) => path)).toEqual([]);
  });
});
