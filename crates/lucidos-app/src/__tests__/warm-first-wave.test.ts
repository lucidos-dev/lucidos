// @ts-expect-error: Node APIs exist where Vitest runs, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { availableParallelism } from 'node:os';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import {
  firstWaveSize, viteEnvironmentOf, warmFirstWave, warmGraph, type Transformer, type WaveSpec,
} from '../../vite/warmFirstWave';

/** A fake Vite environment over a fixed import graph, recording each transform. */
function fakeEnvironment(graph: Record<string, { deps?: string[]; dynamicDeps?: string[] } | Error>) {
  const transformed: string[] = [];
  const env: Transformer = {
    async transformRequest(url) {
      transformed.push(url);
      const node = graph[url];
      if (node instanceof Error) throw node;
      return node ?? null;
    },
  };
  return { env, transformed };
}

describe('viteEnvironmentOf', () => {
  it('serves a jsdom docblock from the client environment', () => {
    expect(viteEnvironmentOf('// @vitest-environment jsdom\nimport x from "y";', 'node')).toBe('client');
  });

  it('falls back to the configured environment without a docblock', () => {
    expect(viteEnvironmentOf('import x from "y";', 'node')).toBe('ssr');
    expect(viteEnvironmentOf('import x from "y";', 'happy-dom')).toBe('client');
  });

  it('serves an environment Vitest does not ship from ssr', () => {
    expect(viteEnvironmentOf('/** @vitest-environment my-custom-env */', 'node')).toBe('ssr');
  });
});

describe('warmGraph', () => {
  it('follows static and dynamic imports once each', async () => {
    const { env, transformed } = fakeEnvironment({
      '/src/a.test.ts': { deps: ['/src/shared.ts'], dynamicDeps: ['/src/store.ts'] },
      '/src/b.test.ts': { deps: ['/src/shared.ts'] },
      '/src/store.ts': { deps: ['/src/shared.ts', '/@fs/repo/packages/sdk/src/x.ts'] },
      '/src/shared.ts': {},
      '/@fs/repo/packages/sdk/src/x.ts': {},
    });
    await warmGraph(env, ['/src/a.test.ts', '/src/b.test.ts']);
    expect(transformed.sort()).toEqual([
      '/@fs/repo/packages/sdk/src/x.ts', '/src/a.test.ts', '/src/b.test.ts', '/src/shared.ts', '/src/store.ts',
    ]);
  });

  it('never transforms externals, virtual ids or node_modules', async () => {
    const { env, transformed } = fakeEnvironment({
      '/src/a.test.ts': { deps: ['preact', 'node:crypto', '/@id/__x00__virtual:build-id', '/repo/node_modules/jsdom/lib/api.js'] },
    });
    await warmGraph(env, ['/src/a.test.ts']);
    expect(transformed).toEqual(['/src/a.test.ts']);
  });

  it('skips a module that fails and keeps warming the rest', async () => {
    const { env, transformed } = fakeEnvironment({
      '/src/a.test.ts': { deps: ['/src/broken.ts', '/src/fine.ts'] },
      '/src/broken.ts': new Error('syntax error'),
      '/src/fine.ts': {},
    });
    await expect(warmGraph(env, ['/src/a.test.ts'])).resolves.toBeUndefined();
    expect(transformed).toContain('/src/fine.ts');
  });
});

describe('firstWaveSize', () => {
  it('is the configured worker count when set', () => {
    expect(firstWaveSize({ maxWorkers: 4, watch: true }, 32)).toBe(4);
  });

  it("mirrors Vitest's pool default when unset: one core spare, or half in watch mode", () => {
    expect(firstWaveSize({ maxWorkers: undefined, watch: false }, 32)).toBe(31);
    expect(firstWaveSize({ maxWorkers: undefined, watch: true }, 32)).toBe(16);
    expect(firstWaveSize({ maxWorkers: undefined, watch: false }, 1)).toBe(1);
    expect(firstWaveSize({ maxWorkers: undefined, watch: true }, 1)).toBe(1);
  });

  it('reads the host core count by default', () => {
    expect(firstWaveSize({ maxWorkers: undefined, watch: false })).toBe(Math.max(availableParallelism() - 1, 1));
  });
});

describe('warmFirstWave', () => {
  it('warms only the first wave, each file in its own environment with the setup files', async () => {
    const ssr = fakeEnvironment({});
    const client = fakeEnvironment({});
    const project = {
      config: { environment: 'node', setupFiles: ['/src/test-setup.ts'] },
      vite: { environments: { ssr: ssr.env, client: client.env } },
    };
    const sources: Record<string, string> = {
      '/src/node.test.ts': 'import x from "y";',
      '/src/dom.test.ts': '// @vitest-environment jsdom',
      '/src/late.test.ts': 'import x from "y";',
    };
    const specs: WaveSpec[] = Object.keys(sources).map((moduleId) => ({ moduleId, project }));
    await warmFirstWave(specs, { maxWorkers: 2, watch: false }, (file) => sources[file]);
    expect(ssr.transformed.sort()).toEqual(['/src/node.test.ts', '/src/test-setup.ts']);
    expect(client.transformed.sort()).toEqual(['/src/dom.test.ts', '/src/test-setup.ts']);
  });

  it('warms a file that vanished after collection in the configured environment', async () => {
    const ssr = fakeEnvironment({});
    const project = {
      config: { environment: 'node', setupFiles: [] },
      vite: { environments: { ssr: ssr.env } },
    };
    const gone = '/nonexistent/removed-after-collection.test.ts';
    await expect(warmFirstWave([{ moduleId: gone, project }], { maxWorkers: 1, watch: false })).resolves.toBeUndefined();
    expect(ssr.transformed).toEqual([gone]);
  });
});

const appFile = (name: string): string =>
  readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '../..', name), 'utf8');

it('only the Vitest config runs the warming sequencer', () => {
  expect(appFile('vitest.config.ts')).toContain('sequencer: WarmFirstWaveSequencer');
  // Vite bundles every relative import of its config, so `vite build` would load `vitest/node`.
  expect(appFile('vite.config.ts')).not.toContain('warmFirstWaveSequencer');
});
