/**
 * Build the shared SSE worker bundle.
 *
 * A `SharedWorker` is constructed from a URL, so this one has to be a
 * self-contained script the engine can serve at
 * `/<slug>/api/v1/sse-worker.js`. It cannot import anything at runtime.
 *
 * The output is CHECKED IN under `src/generated/`, unlike `dist/sdk.js`, for
 * the same reason the appearance boot bundles are: the engine `include_str!`s
 * it, so `cargo build` must not need npm to have run first.
 * `sseWorker.staleness.test.ts` rebuilds and diffs, so a source edit nobody
 * rebuilt fails the ordinary test run instead of shipping the previous worker.
 *
 * `es2020` matches `dist/sdk.js`. Nothing older can reach this script anyway:
 * a browser without `SharedWorker` never requests it and takes the direct
 * `EventSource` fallback instead. `minify` off so the artifact is reviewable.
 */
import { build } from 'esbuild';

/** Entry file to committed artifact. */
export const WORKER_BUNDLE = {
  entry: 'src/worker/sseWorker.ts',
  out: 'src/generated/sse-worker.js',
};

/** Shared config, so the staleness test builds exactly what `npm run build`
 *  wrote. Any drift between the two would make the check meaningless. */
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
  const { writeFile, mkdir } = await import('node:fs/promises');
  await mkdir('src/generated', { recursive: true });
  await writeFile(
    WORKER_BUNDLE.out,
    await buildWorkerBundle(WORKER_BUNDLE.entry),
    'utf8',
  );
}
