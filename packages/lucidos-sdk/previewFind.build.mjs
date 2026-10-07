/**
 * Build the finder bundle the HTML artifact preview runs.
 *
 * The preview frame loads no SDK, so the host stamps this bundle into the
 * document text beside its bridge script (`previewFrameBridge.ts`). The output
 * is CHECKED IN under `src/generated/`, like the SSE worker, because the host
 * imports it as text at build time. `previewFind.staleness.test.ts` rebuilds
 * and diffs, so a matcher edit nobody rebuilt fails the ordinary test run.
 *
 * `globalName` names the IIFE's result. The bridge wraps the bundle in its own
 * function, so that name is a local there, never a window global.
 */
import { build } from 'esbuild';

/** Entry file to committed artifact. */
export const PREVIEW_FIND_BUNDLE = {
  entry: 'src/previewFind/previewFind.ts',
  out: 'src/generated/preview-find.js',
};

/** The local name the bridge reads the finder from. */
export const PREVIEW_FIND_GLOBAL = '__lucidosPreviewFind';

/** Shared config, so the staleness test builds exactly what `npm run build`
 *  wrote. */
export function previewFindBuildOptions(entry) {
  return {
    entryPoints: [entry],
    bundle: true,
    format: 'iife',
    globalName: PREVIEW_FIND_GLOBAL,
    target: 'es2020',
    minify: false,
    sourcemap: false,
    banner: {
      js: '/* GENERATED from packages/lucidos-sdk/src/previewFind/ by previewFind.build.mjs.\n'
        + '   Do not edit: run `npm run build` in packages/lucidos-sdk. */',
    },
  };
}

/** The bundle text, without touching disk. */
export async function buildPreviewFindBundle(entry) {
  const result = await build({ ...previewFindBuildOptions(entry), write: false });
  return result.outputFiles[0].text;
}

export async function writePreviewFindBundle() {
  const { writeFile, mkdir } = await import('node:fs/promises');
  await mkdir('src/generated', { recursive: true });
  await writeFile(
    PREVIEW_FIND_BUNDLE.out,
    await buildPreviewFindBundle(PREVIEW_FIND_BUNDLE.entry),
    'utf8',
  );
}
