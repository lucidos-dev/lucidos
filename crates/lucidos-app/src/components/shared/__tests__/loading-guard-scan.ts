// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';


const here = dirname(fileURLToPath(import.meta.url));
export const SRC = resolve(here, '../../..');

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

export function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1');
}

export type ExemptionTag =
  | 'structure-first'
  | 'single-value'
  | 'working-state'
  | 'no-visual'
  | 'best-effort'
  | 'returns-value';

const TAGS: ReadonlySet<string> = new Set<ExemptionTag>([
  'structure-first', 'single-value', 'working-state', 'no-visual', 'best-effort', 'returns-value',
]);

export interface Exemption {
  tag: ExemptionTag;
  why: string;
}

export function exemptionProblems(
  exempt: Record<string, Exemption>,
  offenders: Set<string>,
  rawSource: (path: string) => string | null,
): string[] {
  const problems: string[] = [];
  for (const [path, { tag, why }] of Object.entries(exempt)) {
    const raw = rawSource(path);
    if (!TAGS.has(tag)) problems.push(`${path}: unknown tag "${tag}"`);
    else if (raw === null) problems.push(`${path}: exempt but missing`);
    else if (!offenders.has(path)) problems.push(`${path}: no longer offends, drop the entry`);
    else if (why.trim() === '') problems.push(`${path}: an exemption states its reason`);
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
