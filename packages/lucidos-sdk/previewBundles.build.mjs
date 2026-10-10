/**
 * Build the bundles the HTML artifact preview runs: the finder and the pane
 * swipe.
 *
 * The preview frame loads no SDK, so the host stamps these bundles into the
 * document text beside its bridge script (`previewFrameBridge.ts`). The output
 * is CHECKED IN under `src/generated/`, like the SSE worker, because the host
 * imports it as text at build time. `previewBundles.staleness.test.ts` rebuilds
 * and diffs, so a source edit nobody rebuilt fails the ordinary test run.
 *
 * `globalName` names each IIFE's result. The bridge wraps the bundles in its
 * own function, so that name is a local there, never a window global.
 */
import { build } from 'esbuild';

/** Each bundle: its entry file, its committed artifact, and the local name
 *  the bridge reads it from. */
export const PREVIEW_BUNDLES = [
  {
    entry: 'src/previewFind/previewFind.ts',
    out: 'src/generated/preview-find.js',
    globalName: '__lucidosPreviewFind',
  },
  {
    entry: 'src/paneSwipe.ts',
    out: 'src/generated/preview-swipe.js',
    globalName: '__lucidosPreviewSwipe',
  },
];

/** Shared config, so the staleness test builds exactly what `npm run build`
 *  wrote. */
export function previewBundleBuildOptions(bundle, entryPath) {
  return {
    entryPoints: [entryPath],
    bundle: true,
    format: 'iife',
    globalName: bundle.globalName,
    target: 'es2020',
    minify: false,
    sourcemap: false,
    banner: {
      js: `/* GENERATED from packages/lucidos-sdk/${bundle.entry} by previewBundles.build.mjs.\n`
        + '   Do not edit: run `npm run build` in packages/lucidos-sdk. */',
    },
  };
}

/** One bundle's text, without touching disk. `entryPath` defaults to the
 *  entry relative to the package root, the cwd of `npm run build`. */
export async function buildPreviewBundle(bundle, entryPath = bundle.entry) {
  const result = await build({ ...previewBundleBuildOptions(bundle, entryPath), write: false });
  return result.outputFiles[0].text;
}

export async function writePreviewBundles() {
  const { writeFile, mkdir } = await import('node:fs/promises');
  await mkdir('src/generated', { recursive: true });
  for (const bundle of PREVIEW_BUNDLES) {
    await writeFile(bundle.out, await buildPreviewBundle(bundle), 'utf8');
  }
}
