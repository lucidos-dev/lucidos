import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';
import { isDevServerBundle, DEV_SERVER_SW_REASON } from './devServerBundle';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '..'); // crates/lucidos-app/src

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) out.push(...sourceFiles(full));
    else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) out.push(full);
  }
  return out;
}

describe('isDevServerBundle', () => {
  it('is true under a Vite dev server, which is what a Vitest run is', () => {
    // Vitest resolves `import.meta.env.DEV` to true, so this assertion doubles
    // as proof the predicate reads the flag rather than a hardcoded constant.
    expect(isDevServerBundle()).toBe(true);
  });
});

describe('DEV_SERVER_SW_REASON', () => {
  it('names the preview and says what the user should do instead', () => {
    // It reaches the user through the push toast, so it has to be actionable
    // rather than a bare "unsupported" (.claude/rules/frontend.md).
    expect(DEV_SERVER_SW_REASON).toMatch(/preview/i);
    expect(DEV_SERVER_SW_REASON).toMatch(/service worker/i);
    expect(DEV_SERVER_SW_REASON).toMatch(/real app/i);
  });
});

describe('service-worker registration is gated on the dev-server check', () => {
  /**
   * The gate is what keeps the frontend preview usable: a Vite dev server
   * serves unhashed module URLs, so a worker that caches them serves the old
   * module after a hot update, which is the entire point of the preview gone.
   *
   * A source scan rather than a render test, because the failure this guards is
   * a THIRD registration site added later without the gate, not a branch in
   * today's code. Same shape as `skeleton-guard.test.ts`.
   */
  const REGISTER = /serviceWorker\s*\.\s*register\s*\(/;

  const registrars = sourceFiles(SRC).filter((f) => REGISTER.test(readFileSync(f, 'utf-8')));

  it('finds the registration sites it expects, so the scan is not vacuous', () => {
    const names = registrars.map((f) => relative(SRC, f)).sort();
    expect(names).toEqual(['hooks/useStartup.ts', 'store/actions/push.ts']);
  });

  it('every file that registers a service worker consults isDevServerBundle', () => {
    for (const file of registrars) {
      expect(
        readFileSync(file, 'utf-8'),
        `${relative(SRC, file)} registers a service worker without the dev-server gate`,
      ).toMatch(/isDevServerBundle/);
    }
  });
});
