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
const APP_SRC = resolve(here, '..');
const SDK_SRC = resolve(REPO_ROOT, 'packages/lucidos-sdk/src');
const INDEX_HTML = resolve(here, '../..', 'index.html');

function stripComments(src: string): string {
  return src
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:])\/\/.*$/gm, '$1');
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

const BOOT_TAB_KEYS: ReadonlySet<string> = new Set([
  'lucidos-splash-mark-formed',
  'lucidos-pwa-workspace-session',
]);
const PWA_RESUME_KEY = 'lucidos-pwa-workspace';

describe('no raw browser storage outside the sanctioned wrappers', () => {
  it('SDK source touches storage ONLY through _storage.ts (separate realm)', () => {
    const offenders: string[] = [];
    for (const file of tsFiles(SDK_SRC)) {
      if (/(^|\/)_storage\.ts$/.test(file)) continue;
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
      if (/(^|\/)workspaceStorage\.ts$/.test(file)) continue;
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
      const after = html.slice(m.index! + m[0].length);
      const arg = after.replace(/^\s*\(\s*/, '');
      if (arg.startsWith('wsKey(')) continue;
      if (m[0] === 'localStorage.setItem' &&
          arg.startsWith(`.bind(localStorage, '${PWA_RESUME_KEY}')`)) continue;
      const lit = arg.replace(/^peerKey\(\s*/, '').match(/^['"]([^'"]+)['"]/);
      if (lit && GLOBAL_KEYS.has(lit[1])) continue;
      if (lit && m[1] === 'sessionStorage' && BOOT_TAB_KEYS.has(lit[1])) continue;
      if (lit?.[1] === PWA_RESUME_KEY && m[0] === 'localStorage.getItem') continue;
      bad.push(lit ? lit[1] : m[0]);
    }
    expect(
      bad,
      `index.html reads/writes storage unscoped (wrap in wsKey() or use a GLOBAL key):\n${bad.join('\n')}`,
    ).toEqual([]);
  });
});
