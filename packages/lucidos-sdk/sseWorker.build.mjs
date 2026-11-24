import { build } from 'esbuild';
import { writeIfChanged } from './writeIfChanged.mjs';

export const WORKER_BUNDLE = {
  entry: 'src/worker/sseWorker.ts',
  out: 'dist/sse-worker.js',
};


export function workerBuildOptions(entry) {
  return {
    entryPoints: [entry],
    bundle: true,
    format: 'iife',
    target: 'es2020',
    minify: false,
    sourcemap: false,
    banner: {
      js: '/* GENERATED from packages/lucidos-sdk/src/worker/ by sseWorker.build.mjs.\n'
        + '   Do not edit: run `npm run build` in packages/lucidos-sdk. */',
    },
  };
}

export async function buildWorkerBundle(entry) {
  const result = await build({ ...workerBuildOptions(entry), write: false });
  return result.outputFiles[0].text;
}

export async function buildSseWorkerBundle() {
  await writeIfChanged(WORKER_BUNDLE.out, await buildWorkerBundle(WORKER_BUNDLE.entry));
}
