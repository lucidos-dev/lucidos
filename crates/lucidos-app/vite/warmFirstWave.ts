/**
 * Warms the module graph of the test files that start together.
 *
 * Every worker fetches its modules through the one Vite server, whose
 * transform cache starts empty on every run. The first `maxWorkers` files
 * start together and would pay every cold transform inside their timed test
 * bodies. The base order puts failed and slow files first, so the same heavy
 * files lost the 5 s test timeout every round. Later files start one at a
 * time and find the shared graph warm. Why not fewer workers: ADR 0754.
 */
// @ts-expect-error: Node APIs exist where Vite runs, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { availableParallelism } from 'node:os';
import { builtinEnvironments } from 'vitest/runtime';

/** The slice of a Vite environment the warm-up uses. */
export interface Transformer {
  transformRequest(url: string): Promise<{ deps?: string[]; dynamicDeps?: string[] } | null>;
}

/** The slice of a Vitest `TestSpecification` the warm-up uses. */
export interface WaveSpec {
  moduleId: string;
  project: {
    config: { environment: string; setupFiles: string[] };
    vite: { environments: Record<string, Transformer | undefined> };
  };
}

const DOCBLOCK_ENVIRONMENT = /@(?:vitest|jest)-environment\s+([\w-]+)/;

/** The Vite environment Vitest serves a test file from, by its docblock. */
export function viteEnvironmentOf(source: string, configured: string): string {
  const name = DOCBLOCK_ENVIRONMENT.exec(source)?.[1] ?? configured;
  return (builtinEnvironments as Record<string, { viteEnvironment?: string }>)[name]?.viteEnvironment ?? 'ssr';
}

/** Project modules only: externals and virtual ids never reach a transform. */
function isProjectModule(url: string): boolean {
  return url.startsWith('/') && !url.startsWith('/@id/') && !url.includes('/node_modules/');
}

/**
 * Transforms every project module reachable from `roots`, static and dynamic.
 * A module that fails is skipped: the test importing it reports the real error.
 */
export async function warmGraph(env: Transformer, roots: readonly string[]): Promise<void> {
  const seen = new Set<string>();
  let frontier = [...roots];
  while (frontier.length > 0) {
    const batch = [...new Set(frontier)].filter((url) => !seen.has(url));
    batch.forEach((url) => seen.add(url));
    const results = await Promise.all(batch.map((url) => env.transformRequest(url).catch(() => null)));
    frontier = results
      .flatMap((result) => [...(result?.deps ?? []), ...(result?.dynamicDeps ?? [])])
      .filter(isProjectModule);
  }
}

/** The worker settings that size the first wave. */
export interface WavePool {
  maxWorkers: number | undefined;
  watch: boolean;
}

/**
 * The number of files Vitest starts together. `maxWorkers` is resolved only
 * when set, so an unset one mirrors Vitest's own pool default.
 */
export function firstWaveSize({ maxWorkers, watch }: WavePool, cores = availableParallelism()): number {
  return maxWorkers ?? Math.max(watch ? Math.floor(cores / 2) : cores - 1, 1);
}

/** A file that cannot be read has no docblock, as in Vitest's own detection. */
function readSourceOrEmpty(file: string): string {
  try {
    return readFileSync(file, 'utf8');
  } catch {
    return '';
  }
}

/** Warms the first wave of `sorted`, each file with its project's setup files. */
export async function warmFirstWave(
  sorted: readonly WaveSpec[],
  pool: WavePool,
  readSource: (file: string) => string = readSourceOrEmpty,
): Promise<void> {
  const roots = new Map<Transformer, string[]>();
  for (const { moduleId, project } of sorted.slice(0, firstWaveSize(pool))) {
    const env = project.vite.environments[viteEnvironmentOf(readSource(moduleId), project.config.environment)];
    if (!env) continue;
    roots.set(env, [...(roots.get(env) ?? project.config.setupFiles), moduleId]);
  }
  await Promise.all([...roots].map(([env, files]) => warmGraph(env, files)));
}
