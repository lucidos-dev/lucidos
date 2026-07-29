import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';

/**
 * The System page's **Client** version must never again be sourced from the
 * engine's CalVer `VERSION` file.
 *
 * It was, via a `virtual:engine-version` Vite plugin that read
 * `crates/lucidos-engine/VERSION` at bundle-build time. The baked value froze at
 * whatever VERSION happened to be when the frontend was last built, while the
 * running engine's VERSION kept bumping on every engine-only Apply — so the
 * Versions section showed two disagreeing numbers (Engine 2026.07.27.1 / Client
 * 2026.07.26.7) that no reload could reconcile, because the loaded bundle already
 * WAS the served bundle. Re-baking on every bump is worse, not better: it makes
 * each engine-only change produce a byte-different bundle → a new sw.js BUILD_ID
 * → a "refresh to sync" toast carrying nothing but a version string, destroying
 * the property that a pure engine-only Switch surfaces nothing.
 *
 * The client's honest identity is its own `CLIENT_BUILD_ID` — the build that
 * produced the running code, and the exact value the refresh badge compares
 * against the served build. The test infra has no jsdom, so this is a
 * source-scan, not a render test (the `skeleton-guard.test.ts` precedent).
 */

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..'); // crates/lucidos-app/src
const APP_DIR = resolve(SRC, '..'); // crates/lucidos-app

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
    // The path appears in the explanatory comment; what must stay gone is a
    // `readFileSync`/`resolve` of it — i.e. an actual plugin reading the file.
    expect(/readFileSync\([^)]*VERSION|resolve\([^)]*lucidos-engine\/VERSION/.test(config)).toBe(
      false,
    );
  });

  it('SystemPage derives the web client version from CLIENT_BUILD_ID', () => {
    const page = readFileSync(resolve(here, 'SystemPage.tsx'), 'utf-8');
    expect(page).toContain("from 'virtual:build-id'");
    // Tauri keeps its real app version (a versioned shell with a real updater);
    // the web fallback is the loaded bundle's build id.
    expect(page).toMatch(/clientVersion\s*=\s*tauriClientVersion\s*\?\?\s*formatBuildId\(CLIENT_BUILD_ID\)/);
  });
});
