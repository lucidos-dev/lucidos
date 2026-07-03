import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync, statSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve, relative } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SRC_ROOT = resolve(__dirname, '..', '..');
const COMPONENTS_ROOT = resolve(SRC_ROOT, 'components');

/** Source roots that export the project's promise-returning surface. The test
 *  discovers async function names from these directories and flags bare calls
 *  to any of them inside `components/**`. */
const ASYNC_SOURCE_ROOTS = [
  resolve(SRC_ROOT, 'api'),
  resolve(SRC_ROOT, 'store'),
  resolve(SRC_ROOT, 'hooks'),
  resolve(SRC_ROOT, 'utils'),
];

/** Component-tree files this lint deliberately skips today. Format:
 *  `['owning sweep / plan', 'components/<rel>.tsx']`. Empty when no debt is
 *  outstanding — that's the steady state. Anything not listed must pass. */
const KNOWN_OFFENDERS_BY_OWNER: ReadonlyArray<readonly [string, string]> = [];
const KNOWN_OFFENDER_PATHS = new Set(KNOWN_OFFENDERS_BY_OWNER.map(([, p]) => p));

function walkTs(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = resolve(dir, entry);
    const stat = statSync(path);
    if (stat.isDirectory()) {
      out.push(...walkTs(path));
    } else if (path.endsWith('.tsx') || path.endsWith('.ts')) {
      out.push(path);
    }
  }
  return out;
}

function isTestFile(path: string): boolean {
  return path.endsWith('.test.ts') ||
    path.endsWith('.test.tsx') ||
    path.includes('/__tests__/');
}

/** Discover the set of identifier names that are exported as promise-returning
 *  functions from `ASYNC_SOURCE_ROOTS`. Catches:
 *    - `export async function X` (any params, any return)
 *    - `export function X(...): Promise<...>` (sync function declaring Promise return)
 *  For the second form we paren-match the parameter list so callback-typed
 *  params like `(cb: () => void): Promise<T>` are still discovered. */
function discoverAsyncExports(): Set<string> {
  const names = new Set<string>();
  for (const root of ASYNC_SOURCE_ROOTS) {
    for (const file of walkTs(root)) {
      if (isTestFile(file)) continue;
      const source = readFileSync(file, 'utf-8');
      for (const m of source.matchAll(/^export async function (\w+)\b/gm)) {
        names.add(m[1]);
      }
      for (const m of source.matchAll(/^export function (\w+)\s*(?:<[^>]*>)?\s*\(/gm)) {
        const openIdx = source.indexOf('(', m.index! + m[0].length - 1);
        const closeIdx = findMatchingParenInSource(source, openIdx);
        if (closeIdx < 0) continue;
        const tail = source.slice(closeIdx + 1, closeIdx + 200);
        if (/^\s*:\s*Promise</.test(tail)) names.add(m[1]);
      }
    }
  }
  return names;
}

/** Like findMatchingParen but for a whole multi-line source — used to walk
 *  past a parameter list whose own parens nest (callback types, defaults). */
function findMatchingParenInSource(s: string, openIdx: number): number {
  let depth = 0;
  let inStr: string | null = null;
  for (let i = openIdx; i < s.length; i++) {
    const ch = s[i];
    if (inStr) {
      if (ch === '\\') { i++; continue; }
      if (ch === inStr) inStr = null;
      continue;
    }
    if (ch === '"' || ch === "'" || ch === '`') {
      inStr = ch;
      continue;
    }
    if (ch === '(') depth++;
    else if (ch === ')') {
      depth--;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/** Find locally-declared async function names in a component source. Bare
 *  calls to a local helper inside the same file are just as floating as bare
 *  calls to an imported one. Catches three forms:
 *    - `async function NAME(...) { ... }`
 *    - `const/let/var NAME = async (...) => ...`
 *    - `const/let/var NAME = useCallback(async (...) => ..., [...])`
 *  TSX components write helpers in all three shapes; the keyword-only regex
 *  missed the latter two. */
function discoverLocalAsync(source: string): Set<string> {
  const names = new Set<string>();
  for (const m of source.matchAll(/\basync function (\w+)\b/g)) {
    names.add(m[1]);
  }
  for (const m of source.matchAll(/\b(?:const|let|var)\s+(\w+)\s*(?::\s*[^=]+)?=\s*async\s*[\(<]/g)) {
    names.add(m[1]);
  }
  for (const m of source.matchAll(/\b(?:const|let|var)\s+(\w+)\s*(?::\s*[^=]+)?=\s*useCallback\s*\(\s*async\b/g)) {
    names.add(m[1]);
  }
  return names;
}

/** Find the index of the `)` that closes the `(` at `openIdx`. Tracks string
 *  literals (single/double/backtick) so paren-looking chars inside strings
 *  don't throw off the depth counter. Returns -1 if the `(` doesn't close on
 *  the same line. */
function findMatchingParen(s: string, openIdx: number): number {
  let depth = 0;
  let inStr: string | null = null;
  for (let i = openIdx; i < s.length; i++) {
    const ch = s[i];
    if (inStr) {
      if (ch === '\\') { i++; continue; }
      if (ch === inStr) inStr = null;
      continue;
    }
    if (ch === '"' || ch === "'" || ch === '`') {
      inStr = ch;
      continue;
    }
    if (ch === '(') depth++;
    else if (ch === ')') {
      depth--;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/** Receivers that consume promises themselves. A bare-looking call passed as
 *  their first argument (`useLoadableFetch(() => name())`) is not floating —
 *  the receiver awaits / chains it. Listed by exact identifier name. */
const PROMISE_CONSUMING_RECEIVERS = ['useLoadableFetch'];

/** True if the call at `lines[i]` is the body of an arrow that is itself the
 *  first argument to a known promise-consuming receiver on the preceding line.
 *  Pattern: `useLoadableFetch<...>(` ends the previous line; this line begins
 *  with `() => name(...)`. */
function isInsideAsyncConsumer(lines: string[], i: number): boolean {
  for (let j = i - 1; j >= 0; j--) {
    const ln = lines[j].trimEnd();
    if (ln === '') continue;
    for (const receiver of PROMISE_CONSUMING_RECEIVERS) {
      if (new RegExp(`\\b${receiver}(?:\\s*<[^>]*>)?\\s*\\(\\s*$`).test(ln)) {
        return true;
      }
    }
    return false;
  }
  return false;
}

/** True if the closing paren of the call ends the current line and the next
 *  non-blank line begins with `.then(` / `.catch(` / `.finally(` — i.e. the
 *  promise is chained across lines, not floated. */
function nextNonBlankLineStartsWithChain(lines: string[], i: number): boolean {
  for (let j = i + 1; j < lines.length; j++) {
    const ln = lines[j].trim();
    if (ln === '') continue;
    return /^\.\s*(then|catch|finally)\s*\(/.test(ln);
  }
  return false;
}

/** Match a bare call `<name>(...)` that isn't awaited, voided, returned,
 *  thrown, assigned, passed as an argument, or chained with `.then/.catch/
 *  .finally`. Definition lines, import lines, and prop assignments are
 *  filtered out. Multi-line continuation lines that start with `?`/`:`/`,`
 *  are treated as part of the parent expression, not bare statements. */
function findFloatingCalls(source: string, name: string): string[] {
  const lines = source.split('\n');
  const offenders: string[] = [];
  const pattern = new RegExp(`(^|[^a-zA-Z_$.])${name}\\s*\\(`);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!pattern.test(line)) continue;
    // Comment-only lines and JSDoc-style continuation lines never call code.
    if (/^\s*(\/\/|\*|\/\*)/.test(line)) continue;
    if (new RegExp(`\\b(function|async function)\\s+${name}\\b`).test(line)) continue;
    if (/^\s*import\b/.test(line)) continue;
    if (new RegExp(`\\b${name}\\s*:`).test(line)) continue;
    if (new RegExp(`(const|let|var)\\s+${name}\\b`).test(line)) continue;
    const trimmed = line.trimStart();
    // Ternary / argument-list continuations are never bare statements.
    if (/^[?:,]/.test(trimmed)) continue;
    const callIdx = line.search(new RegExp(`(^|[^a-zA-Z_$.])${name}\\s*\\(`));
    const before = line.slice(0, callIdx + 1);
    if (/\b(void|await|return|throw)\s*$/.test(before)) continue;
    if (/\.\s*(then|catch|finally)\s*\(\s*$/.test(before)) continue;
    if (/=\s*$/.test(before)) continue;
    if (/\(\s*$/.test(before)) continue;
    if (/,\s*$/.test(before)) continue;

    // What comes after the call's closing paren on the same line? If it's
    // `.then(` / `.catch(` / `.finally(`, the promise is chained and fine.
    const openParenIdx = line.indexOf('(', callIdx);
    if (openParenIdx >= 0) {
      const closeParenIdx = findMatchingParen(line, openParenIdx);
      if (closeParenIdx >= 0) {
        const after = line.slice(closeParenIdx + 1).trimStart();
        if (/^\.\s*(then|catch|finally)\s*\(/.test(after)) continue;
        // Closing paren ends the line — check next line for the chain.
        if (after === '' && nextNonBlankLineStartsWithChain(lines, i)) continue;
      }
    }

    // The receiver above this line might be a promise-consuming hook (e.g.
    // `useLoadableFetch(\n  () => name(...),\n  [...])`). Don't flag those.
    if (isInsideAsyncConsumer(lines, i)) continue;

    offenders.push(`${i + 1}: ${line.trim()}`);
  }
  return offenders;
}

const ASYNC_EXPORTS = discoverAsyncExports();
const COMPONENT_FILES = walkTs(COMPONENTS_ROOT).filter(f => !isTestFile(f));

describe('no floating promises in components/', () => {
  // Sanity-check the discovery — if the source roots ever drift out from under
  // us, the test loses its teeth silently. A bare assertion here makes that
  // failure mode loud.
  it('discovers a plausible set of async export names', () => {
    expect(ASYNC_EXPORTS.size).toBeGreaterThan(50);
    // Spot-check known async exports across action/api/v1/preferences surfaces.
    expect(ASYNC_EXPORTS.has('loadApps')).toBe(true);
    expect(ASYNC_EXPORTS.has('uploadFiles')).toBe(true);
    expect(ASYNC_EXPORTS.has('applySingleChange')).toBe(true);
    expect(ASYNC_EXPORTS.has('setTheme')).toBe(true);
    expect(ASYNC_EXPORTS.has('setUiScale')).toBe(true);
    // `listen` in utils/tauri.ts has a callback-typed parameter
    // (`handler: (e: { payload: T }) => void`) — verifies the paren-matching
    // in `discoverAsyncExports` walks past the inner `)`.
    expect(ASYNC_EXPORTS.has('listen')).toBe(true);
  });

  for (const file of COMPONENT_FILES) {
    const rel = relative(SRC_ROOT, file);
    if (KNOWN_OFFENDER_PATHS.has(rel)) continue;
    it(`${rel} has no bare async calls`, () => {
      const source = readFileSync(file, 'utf-8');
      const localAsync = discoverLocalAsync(source);
      const allNames = new Set<string>([...ASYNC_EXPORTS, ...localAsync]);
      const offenders: string[] = [];
      for (const name of allNames) {
        const found = findFloatingCalls(source, name);
        for (const o of found) offenders.push(`${name} → ${o}`);
      }
      expect(offenders).toEqual([]);
    });
  }

  // The exclusion list is a debt marker, not a permanent skip. Each entry
  // must still be a real file (typo guard) so a rename surfaces the entry
  // for cleanup instead of letting it rot.
  it('every known-offender path still exists', () => {
    for (const [, p] of KNOWN_OFFENDERS_BY_OWNER) {
      const abs = resolve(SRC_ROOT, p);
      expect(statSync(abs).isFile()).toBe(true);
    }
  });
});
