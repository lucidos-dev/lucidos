import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';
import { isDevServerBundle, DEV_SERVER_SW_REASON } from './devServerBundle';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '..');

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
    expect(isDevServerBundle()).toBe(true);
  });
});

describe('DEV_SERVER_SW_REASON', () => {
  it('names the preview and says what the user should do instead', () => {
    expect(DEV_SERVER_SW_REASON).toMatch(/preview/i);
    expect(DEV_SERVER_SW_REASON).toMatch(/service worker/i);
    expect(DEV_SERVER_SW_REASON).toMatch(/real app/i);
  });
});

describe('service-worker registration is gated on the dev-server check', () => {
  const REGISTER = /serviceWorker\s*\.\s*register\s*\(/;

  const registrars = sourceFiles(SRC).filter((f) => REGISTER.test(readFileSync(f, 'utf-8')));

  it('finds the registration sites it expects, so the scan is not vacuous', () => {
    const names = registrars.map((f) => relative(SRC, f)).sort();
    expect(names).toEqual(['store/actions/push.ts', 'store/startup.ts']);
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
