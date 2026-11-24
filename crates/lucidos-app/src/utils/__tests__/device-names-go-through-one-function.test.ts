import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';


const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..');
const THE_FUNCTION = 'utils/deviceFriendlyName.ts';

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== '__tests__' && entry.name !== 'generated') out.push(...sourceFiles(full));
    } else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

const BUILT_NAME = /`device-\$\{/;
const RAW_RECORDED_LABEL = /\{\s*(?:actor|origin)\??\.label\s*\}/;

describe('device names go through one function', () => {
  const files = sourceFiles(SRC).map((f) => ({ path: relative(SRC, f), text: readFileSync(f, 'utf8') }));

  it('scans the tree it claims to', () => {
    expect(files.some((f) => f.path === THE_FUNCTION)).toBe(true);
  });

  it('builds `device-<id>` only in the function itself', () => {
    const offenders = files
      .filter((f) => f.path !== THE_FUNCTION && BUILT_NAME.test(f.text))
      .map((f) => f.path);
    expect(offenders).toEqual([]);
  });

  it("never prints a turn's recorded device label", () => {
    const offenders = files.filter((f) => RAW_RECORDED_LABEL.test(f.text)).map((f) => f.path);
    expect(offenders).toEqual([]);
  });
});
