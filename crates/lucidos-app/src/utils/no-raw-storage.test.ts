/**
 * Permanent regression guard for per-workspace storage scoping.
 *
 * The gateway serves every workspace from one origin (ADR 0014), so any
 * unscoped `localStorage`/`sessionStorage` key bleeds across workspaces. The
 * scoping is enforced three ways across three realms — and THIS test fails the
 * build the moment any of them is bypassed, so the bug class stays closed:
 *
 *   1. Main app realm — `workspaceStorage.ts` overrides `Storage.prototype`, so
 *      every `localStorage.*` call is auto-namespaced. The only bypass is
 *      grabbing the raw prototype methods, so `Storage.prototype` /
 *      `getPrototypeOf` may appear ONLY in `workspaceStorage.ts`.
 *   2. SDK / iframe realm — a SEPARATE realm the override can't reach, so the
 *      SDK must route ALL storage through `_storage.ts`. No other SDK source
 *      file may touch `localStorage`/`sessionStorage` directly.
 *   3. Boot scripts — `index.html` runs before the override installs, so every
 *      storage call there must be `wsKey(...)`-wrapped or use a global picker
 *      key (`GLOBAL_KEYS`).
 *
 * (The allowlist itself — exactly the two cross-workspace picker keys — is
 * locked by `workspaceStorage.test.ts`.)
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
// @ts-expect-error — same
import { dirname, resolve, relative } from 'node:path';
import { GLOBAL_KEYS } from './workspaceStorage';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../..');
const APP_SRC = resolve(here, '..'); // crates/lucidos-app/src
const SDK_SRC = resolve(REPO_ROOT, 'packages/lucidos-sdk/src');
const INDEX_HTML = resolve(here, '../..', 'index.html');

/** Strip `//` line comments and `/* *​/` block comments so a prose mention of
 *  "localStorage." in a doc comment never trips the scanners. Crude but
 *  sufficient: storage API tokens never appear inside string literals here. */
function stripComments(src: string): string {
  return src
    .replace(/<!--[\s\S]*?-->/g, '') // HTML comments (index.html prose)
    .replace(/\/\*[\s\S]*?\*\//g, '') // block comments
    .replace(/(^|[^:])\/\/.*$/gm, '$1'); // line comments; keep the `:` in `https://`
}

function tsFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...tsFiles(full));
    } else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

const STORAGE_API = /\b(localStorage|sessionStorage)\.(getItem|setItem|removeItem|clear|key|length)\b/;
const STORAGE_API_G = new RegExp(STORAGE_API, 'g');

/**
 * Boot-script keys that describe THIS TAB, not this workspace, so namespacing
 * them by workspace would be wrong rather than merely unnecessary. Distinct from
 * `GLOBAL_KEYS` (the app realm's localStorage picker allowlist): nothing outside
 * the inline boot scripts and the gateway splash ever reads these.
 *
 *  - `lucidos-splash-mark-formed`: the gateway boot splash hands the workspace
 *    document a "the mark is already built and standing on screen" flag
 *    (crates/lucidos-gateway/src/proxy.rs), so the reveal is not replayed at the
 *    swap. It states what the tab last PAINTED, which is true whichever
 *    workspace the next document belongs to, and index.html removes it as it
 *    reads it, so it cannot outlive that one navigation.
 */
const BOOT_TAB_KEYS: ReadonlySet<string> = new Set(['lucidos-splash-mark-formed']);

describe('no raw browser storage outside the sanctioned wrappers', () => {
  it('SDK source touches storage ONLY through _storage.ts (separate realm)', () => {
    const offenders: string[] = [];
    for (const file of tsFiles(SDK_SRC)) {
      if (/(^|\/)_storage\.ts$/.test(file)) continue; // the sanctioned helper
      const code = stripComments(readFileSync(file, 'utf8'));
      if (STORAGE_API.test(code)) offenders.push(relative(REPO_ROOT, file));
    }
    expect(
      offenders,
      `SDK files use raw localStorage/sessionStorage — route them through _storage.ts:\n${offenders.join('\n')}`,
    ).toEqual([]);
  });

  it('main app realm never grabs the raw Storage prototype (bypasses the override)', () => {
    const offenders: string[] = [];
    for (const file of tsFiles(APP_SRC)) {
      if (/(^|\/)workspaceStorage\.ts$/.test(file)) continue; // the override itself
      const code = stripComments(readFileSync(file, 'utf8'));
      if (/\bStorage\.prototype\b/.test(code) || /\bgetPrototypeOf\s*\(/.test(code)) {
        offenders.push(relative(REPO_ROOT, file));
      }
    }
    expect(
      offenders,
      `These files reach for the raw Storage prototype, bypassing per-workspace scoping:\n${offenders.join('\n')}`,
    ).toEqual([]);
  });

  it('index.html boot scripts namespace every per-workspace storage call', () => {
    const html = stripComments(readFileSync(INDEX_HTML, 'utf8'));
    const bad: string[] = [];
    for (const m of html.matchAll(STORAGE_API_G)) {
      // Look at the argument that follows the `(` of this call.
      const after = html.slice(m.index! + m[0].length);
      const arg = after.replace(/^\s*\(\s*/, '');
      if (arg.startsWith('wsKey(')) continue; // namespaced by hand — OK
      const lit = arg.match(/^['"]([^'"]+)['"]/);
      if (lit && GLOBAL_KEYS.has(lit[1])) continue; // cross-workspace picker key — OK raw
      if (lit && BOOT_TAB_KEYS.has(lit[1])) continue; // per-TAB state, OK raw
      bad.push(lit ? lit[1] : m[0]);
    }
    expect(
      bad,
      `index.html reads/writes storage unscoped (wrap in wsKey() or use a GLOBAL key):\n${bad.join('\n')}`,
    ).toEqual([]);
  });
});
