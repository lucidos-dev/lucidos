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

const ASYNC_SOURCE_ROOTS = [
  resolve(SRC_ROOT, 'api'),
  resolve(SRC_ROOT, 'store'),
  resolve(SRC_ROOT, 'hooks'),
  resolve(SRC_ROOT, 'utils'),
];

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

function discoverAsyncExports(): Set<string> {
  const names = new Set<string>();
  for (const root of ASYNC_SOURCE_ROOTS) {
    for (const file of walkTs(root)) {
      if (isTestFile(file)) continue;
      for (const name of asyncExportNames(readFileSync(file, 'utf-8'))) names.add(name);
    }
  }
  return names;
}

function asyncExportNames(source: string): string[] {
  const names: string[] = [];
  for (const m of source.matchAll(/^export async function (\w+)\b/gm)) {
    names.push(m[1]);
  }
  for (const m of source.matchAll(/^export function (\w+)\s*(?:<[^>]*>)?\s*\(/gm)) {
    const openIdx = source.indexOf('(', m.index! + m[0].length - 1);
    const closeIdx = findMatchingParen(source, openIdx);
    if (closeIdx < 0) continue;
    const tail = source.slice(closeIdx + 1, closeIdx + 200);
    if (/^\s*:\s*Promise</.test(tail)) names.push(m[1]);
  }
  return names;
}

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
    if (ch === '/' && (s[i + 1] === '/' || s[i + 1] === '*')) {
      const end = s[i + 1] === '/' ? s.indexOf('\n', i) : s.indexOf('*/', i + 2);
      if (end < 0) return -1;
      i = s[i + 1] === '/' ? end : end + 1;
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

function isOpenArgListElement(lines: string[], i: number): boolean {
  for (let j = i - 1; j >= 0; j--) {
    const ln = lines[j].trimEnd();
    if (ln === '' || /^\s*(\/\/|\*|\/\*)/.test(ln)) continue;
    return /[([,]$/.test(ln);
  }
  return false;
}

function nextNonBlankLineStartsWithChain(lines: string[], i: number): boolean {
  for (let j = i + 1; j < lines.length; j++) {
    const ln = lines[j].trim();
    if (ln === '') continue;
    return /^\.\s*(then|catch|finally)\s*\(/.test(ln);
  }
  return false;
}

function findFloatingCalls(source: string, name: string): string[] {
  const lines = source.split('\n');
  const offenders: string[] = [];
  const pattern = new RegExp(`(^|[^a-zA-Z_$.])${name}\\s*\\(`);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!pattern.test(line)) continue;
    if (/^\s*(\/\/|\*|\/\*)/.test(line)) continue;
    if (new RegExp(`\\b(function|async function)\\s+${name}\\b`).test(line)) continue;
    if (/^\s*import\b/.test(line)) continue;
    if (new RegExp(`\\b${name}\\s*:`).test(line)) continue;
    if (new RegExp(`(const|let|var)\\s+${name}\\b`).test(line)) continue;
    const trimmed = line.trimStart();
    if (/^[?:,]/.test(trimmed)) continue;
    const callIdx = line.search(new RegExp(`(^|[^a-zA-Z_$.])${name}\\s*\\(`));
    const before = line.slice(0, callIdx + 1);
    if (/\b(void|await|return|throw)\s*$/.test(before)) continue;
    if (/\.\s*(then|catch|finally)\s*\(\s*$/.test(before)) continue;
    if (/=\s*$/.test(before)) continue;
    if (/\(\s*$/.test(before)) continue;
    if (/,\s*$/.test(before)) continue;

    const openParenIdx = line.indexOf('(', callIdx);
    if (openParenIdx >= 0) {
      const closeParenIdx = findMatchingParen(line, openParenIdx);
      if (closeParenIdx >= 0) {
        const after = line.slice(closeParenIdx + 1).trimStart();
        if (/^\.\s*(then|catch|finally)\s*\(/.test(after)) continue;
        if (after === '' && nextNonBlankLineStartsWithChain(lines, i)) continue;
      }
    }

    if (isOpenArgListElement(lines, i)) continue;

    offenders.push(`${i + 1}: ${line.trim()}`);
  }
  return offenders;
}

const ASYNC_EXPORTS = discoverAsyncExports();
const COMPONENT_FILES = walkTs(COMPONENTS_ROOT).filter(f => !isTestFile(f));

describe('no floating promises in components/', () => {
  it('discovers a plausible set of async export names', () => {
    expect(ASYNC_EXPORTS.size).toBeGreaterThan(50);
    expect(ASYNC_EXPORTS.has('loadApps')).toBe(true);
    expect(ASYNC_EXPORTS.has('uploadFiles')).toBe(true);
    expect(ASYNC_EXPORTS.has('applySingleChange')).toBe(true);
    expect(ASYNC_EXPORTS.has('setThemeMode')).toBe(true);
    expect(ASYNC_EXPORTS.has('setUiScale')).toBe(true);
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

  describe('the parameter-list walk', () => {
    it('walks past a quote inside a comment', () => {
      const source = [
        'export function open(nav: {',
        "  /** The reader's place (if any). */",
        "  fragment?: string; // absent is not empty",
        '}): Promise<unknown> | void {',
        '}',
      ].join('\n');
      expect(asyncExportNames(source)).toEqual(['open']);
    });

    it('still reads a paren or a slash pair inside a string as text', () => {
      const source = "export function open(sep = ')', url = 'a//b'): Promise<void> {\n}";
      expect(asyncExportNames(source)).toEqual(['open']);
    });
  });

  describe('the multi-line argument-list exemption', () => {
    it('accepts an element of an argument list opened on an earlier line', () => {
      const source = [
        'async function go() {',
        '  const [a, b] = await Promise.allSettled([',
        '    getThing(),',
        '    getOther(),',
        '  ]);',
        '}',
      ].join('\n');
      expect(findFloatingCalls(source, 'getThing')).toEqual([]);
      expect(findFloatingCalls(source, 'getOther')).toEqual([]);
    });

    it('still flags a float on the line after the list closes', () => {
      const source = [
        'async function go() {',
        '  await Promise.allSettled([',
        '    getThing(),',
        '  ]);',
        '  getOther();',
        '}',
      ].join('\n');
      expect(findFloatingCalls(source, 'getThing')).toEqual([]);
      expect(findFloatingCalls(source, 'getOther')).toHaveLength(1);
    });

    it('covers the promise-consuming hook shape it replaced', () => {
      const source = [
        'function C() {',
        '  const { loadable } = useLoadableFetch<string>(',
        '    () => readAppSource(app.id),',
        '    [app.id],',
        '  );',
        '}',
      ].join('\n');
      expect(findFloatingCalls(source, 'readAppSource')).toEqual([]);
    });

    it('still flags consecutive bare statements', () => {
      const source = ['function go() {', '  getThing();', '  getOther();', '}'].join('\n');
      expect(findFloatingCalls(source, 'getThing')).toHaveLength(1);
      expect(findFloatingCalls(source, 'getOther')).toHaveLength(1);
    });
  });

  it('every known-offender path still exists', () => {
    for (const [, p] of KNOWN_OFFENDERS_BY_OWNER) {
      const abs = resolve(SRC_ROOT, p);
      expect(statSync(abs).isFile()).toBe(true);
    }
  });
});
