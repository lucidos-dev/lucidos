import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';


const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..');
const APP_DIR = resolve(SRC, '..');

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) out.push(...sourceFiles(full));
    else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) out.push(full);
  }
  return out;
}

describe('client version source', () => {
  it('no module imports the engine CalVer into the bundle', () => {
    const offenders = sourceFiles(SRC).filter((f) =>
      readFileSync(f, 'utf-8').includes('virtual:engine-version'),
    );
    expect(
      offenders,
      'the engine VERSION must not be baked into the client bundle — it freezes at ' +
        'bundle-build time and drifts from the running engine (see vite.config.ts)',
    ).toEqual([]);
  });

  it('the Vite config does not read the engine VERSION file', () => {
    const config = readFileSync(resolve(APP_DIR, 'vite.config.ts'), 'utf-8');
    expect(/readFileSync\([^)]*VERSION|resolve\([^)]*lucidos-engine\/VERSION/.test(config)).toBe(
      false,
    );
  });

  it('the client version derives from CLIENT_BUILD_ID, in one place', () => {
    const util = readFileSync(resolve(SRC, 'utils/clientVersion.ts'), 'utf-8');
    expect(util).toContain("from 'virtual:build-id'");
    expect(util).toMatch(/window\.__LUCIDOS_APP_VERSION__/);
    expect(util).toMatch(/formatBuildId\(CLIENT_BUILD_ID\)/);

    const page = readFileSync(resolve(here, 'SystemPage.tsx'), 'utf-8');
    expect(page).toMatch(/clientVersion\s*=\s*clientVersionLabel\(\)/);
  });
});
