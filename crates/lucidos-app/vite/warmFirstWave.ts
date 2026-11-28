// @ts-expect-error: Node APIs exist where Vite runs, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { availableParallelism } from 'node:os';
import { builtinEnvironments } from 'vitest/runtime';

export interface Transformer {
  transformRequest(url: string): Promise<{ deps?: string[]; dynamicDeps?: string[] } | null>;
}

export interface WaveSpec {
  moduleId: string;
  project: {
    config: { environment: string; setupFiles: string[] };
    vite: { environments: Record<string, Transformer | undefined> };
  };
}

const DOCBLOCK_ENVIRONMENT = /@(?:vitest|jest)-environment\s+([\w-]+)/;

export function viteEnvironmentOf(source: string, configured: string): string {
  const name = DOCBLOCK_ENVIRONMENT.exec(source)?.[1] ?? configured;
  return (builtinEnvironments as Record<string, { viteEnvironment?: string }>)[name]?.viteEnvironment ?? 'ssr';
}

function isProjectModule(url: string): boolean {
  return url.startsWith('/') && !url.startsWith('/@id/') && !url.includes('/node_modules/');
}

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

export interface WavePool {
  maxWorkers: number | undefined;
  watch: boolean;
}

export function firstWaveSize({ maxWorkers, watch }: WavePool, cores = availableParallelism()): number {
  return maxWorkers ?? Math.max(watch ? Math.floor(cores / 2) : cores - 1, 1);
}

function readSourceOrEmpty(file: string): string {
  try {
    return readFileSync(file, 'utf8');
  } catch {
    return '';
  }
}

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
