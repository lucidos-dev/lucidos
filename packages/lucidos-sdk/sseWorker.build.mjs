/**
 * Build the shared SSE worker bundle.
 *
 * A `SharedWorker` is constructed from a URL, so this one has to be a
 * self-contained script the engine can serve at
 * `/<slug>/api/v1/sse-worker.js`. It cannot import anything at runtime.
 *
 * The output lands in `dist/`, beside `sdk.js`, and is never committed. The
 * engine reads it at startup, like the appearance boot bundle.
 *
 * `es2020` matches `dist/sdk.js`. Nothing older can reach this script anyway:
 * a browser without `SharedWorker` never requests it and takes the direct
 * `EventSource` fallback instead. `minify` off so the artifact is reviewable.
 */
import { build } from 'esbuild';
import { writeIfChanged } from './writeIfChanged.mjs';

/** Entry file to build output. */
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

/** The bundle text, without touching disk. */
export async function buildWorkerBundle(entry) {
  const result = await build({ ...workerBuildOptions(entry), write: false });
  return result.outputFiles[0].text;
}

export async function buildSseWorkerBundle() {
  await writeIfChanged(WORKER_BUNDLE.out, await buildWorkerBundle(WORKER_BUNDLE.entry));
}
