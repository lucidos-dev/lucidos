/**
 * The transcript's scroll engine ships in the shell chunk, never the entry.
 * The store reaches it through `components/chat/scrollStateHandle.ts`, which
 * `startShell` fills. One static import of `scrollState` from anything
 * `main.tsx` reaches puts all of it back on first paint, about 22 kB.
 *
 * This walks the static import graph from `main.tsx`, as Rollup does for the
 * entry chunk. A dynamic `import()` starts another chunk and is not followed,
 * and neither is a type-only import, which compiles away.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { existsSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

const SRC = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** `import … from '…'`, `export … from '…'` and a bare `import '…'`. */
const STATIC_IMPORT = /^\s*(import|export)\s+(type\s+)?(?:([^'";=]*?)\s+from\s+)?['"]([^'"]+)['"]/gm;

function isTypeOnly(typeKeyword: string | undefined, clause: string | undefined): boolean {
  if (typeKeyword) return true;
  const named = clause?.trim().match(/^\{([\s\S]*)\}$/);
  if (!named) return false;
  const specifiers = named[1].split(',').map((s: string) => s.trim()).filter(Boolean);
  return specifiers.length > 0 && specifiers.every((s: string) => s.startsWith('type '));
}

function resolveSource(from: string, spec: string): string | null {
  if (!spec.startsWith('.')) return null;
  const base = resolve(dirname(from), spec);
  for (const candidate of [base, `${base}.ts`, `${base}.tsx`, `${base}/index.ts`, `${base}/index.tsx`]) {
    if (/\.tsx?$/.test(candidate) && existsSync(candidate)) return candidate;
  }
  return null;
}

/** Every module `root` reaches by static imports, each with the module that first imported it. */
function staticGraph(root: string): Map<string, string | null> {
  const parent = new Map<string, string | null>([[root, null]]);
  const queue = [root];
  while (queue.length > 0) {
    const file = queue.shift()!;
    const source = readFileSync(file, 'utf8');
    for (const [, , typeKeyword, clause, spec] of source.matchAll(STATIC_IMPORT)) {
      if (isTypeOnly(typeKeyword, clause)) continue;
      const target = resolveSource(file, spec);
      if (!target || parent.has(target)) continue;
      parent.set(target, file);
      queue.push(target);
    }
  }
  return parent;
}

function chainTo(graph: Map<string, string | null>, file: string): string {
  const chain: string[] = [];
  for (let at: string | null | undefined = file; at; at = graph.get(at)) chain.unshift(relative(SRC, at));
  return chain.join(' -> ');
}

describe('the entry chunk leaves the scroll engine to the shell', () => {
  const graph = staticGraph(resolve(SRC, 'main.tsx'));
  const scrollState = resolve(SRC, 'components/chat/scrollState.ts');

  it('reaches the store and the handle, so the walk itself works', () => {
    expect(graph.has(resolve(SRC, 'store/actions/threads.ts'))).toBe(true);
    expect(graph.has(resolve(SRC, 'components/chat/scrollStateHandle.ts'))).toBe(true);
  });

  it('never reaches scrollState.ts by a static import', () => {
    expect(graph.has(scrollState), graph.has(scrollState) ? chainTo(graph, scrollState) : '').toBe(false);
  });
});
