// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

/** Shared plumbing for the two loading guards (`skeleton-guard.test.ts` and
 *  `loadable-guard.test.ts`). Both are source scans: default-deny, with a
 *  closed exemption list whose every entry names the rule that exempts it. */

const here = dirname(fileURLToPath(import.meta.url));
export const SRC = resolve(here, '../../..'); // crates/lucidos-app/src

/** Non-test sources under `src/<dir>`, as `{ path, code }` with `path`
 *  relative to `src` and comments stripped from `code`. */
export function scanSources(dir: string, exts: string[]): { path: string; code: string }[] {
  const walk = (d: string): string[] =>
    readdirSync(d, { withFileTypes: true }).flatMap((e: { name: string; isDirectory(): boolean }) => {
      const full = resolve(d, e.name);
      if (e.isDirectory()) return e.name === '__tests__' ? [] : walk(full);
      const wanted = exts.some((x) => e.name.endsWith(x)) && !/\.test\.tsx?$/.test(e.name);
      return wanted ? [full] : [];
    });
  return walk(resolve(SRC, dir)).map((f: string) => ({
    path: relative(SRC, f),
    code: stripComments(readFileSync(f, 'utf8')),
  }));
}

/** Drops block and line comments, so prose that NAMES a banned form (a
 *  comment explaining why "Loading…" was removed) never trips a guard. A
 *  `//` right after a colon is a URL scheme and stays. */
export function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1');
}

/** Why an exempt file may skip the rule. Closed set: an entry with any other
 *  tag fails the guard. */
export type ExemptionTag =
  | 'structure-first' // anchored popover, or a control that draws its real structure at once and defers only values
  | 'single-value' // one value inside otherwise-real markup; its slot stays empty
  | 'working-state' // an indeterminate in-flight action, which keeps a spinner
  | 'no-visual' // the Loadable only feeds logic, e.g. a label lookup
  | 'best-effort' // frontend.md's telemetry carve-out; the file must say so
  | 'returns-value'; // the read is returned to its caller, never held as state

const TAGS: ReadonlySet<string> = new Set<ExemptionTag>([
  'structure-first', 'single-value', 'working-state', 'no-visual', 'best-effort', 'returns-value',
]);

export interface Exemption {
  tag: ExemptionTag;
  why: string;
}

/** What is wrong with an exemption list. An entry goes wrong in four ways:
 *  - its tag is outside the closed set;
 *  - its file no longer exists;
 *  - its file no longer offends, so the entry is stale;
 *  - it is `best-effort`, but the file lacks frontend.md's carve-out comment. */
export function exemptionProblems(
  exempt: Record<string, Exemption>,
  offenders: Set<string>,
  rawSource: (path: string) => string | null,
): string[] {
  const problems: string[] = [];
  for (const [path, { tag }] of Object.entries(exempt)) {
    const raw = rawSource(path);
    if (!TAGS.has(tag)) problems.push(`${path}: unknown tag "${tag}"`);
    else if (raw === null) problems.push(`${path}: exempt but missing`);
    else if (!offenders.has(path)) problems.push(`${path}: no longer offends, drop the entry`);
    else if (tag === 'best-effort' && !/carve-out/i.test(raw)) {
      problems.push(`${path}: best-effort needs the frontend.md carve-out comment`);
    }
  }
  return problems;
}

export function rawSourceOf(path: string): string | null {
  try {
    return readFileSync(resolve(SRC, path), 'utf8');
  } catch {
    return null;
  }
}
