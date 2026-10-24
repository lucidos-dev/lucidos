/**
 * Build the bundles the HTML artifact preview runs: the finder and the pane
 * swipe.
 *
 * The preview frame loads no SDK, so the host stamps these bundles into the
 * document text beside its bridge script (`previewFrameBridge.ts`). The output
 * lands in `dist/`, like the SSE worker, and the host imports it as text at
 * build time.
 *
 * `globalName` names each IIFE's result. The bridge wraps the bundles in its
 * own function, so that name is a local there, never a window global.
 */
import { build } from 'esbuild';
import { writeIfChanged } from './writeIfChanged.mjs';

/** Each bundle: its entry file, its build output, and the local name the
 *  bridge reads it from. */
export const PREVIEW_BUNDLES = [
  {
    entry: 'src/previewFind/previewFind.ts',
    out: 'dist/preview-find.js',
    globalName: '__lucidosPreviewFind',
  },
  {
    entry: 'src/paneSwipe.ts',
    out: 'dist/preview-swipe.js',
    globalName: '__lucidosPreviewSwipe',
  },
];

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
  for (const bundle of PREVIEW_BUNDLES) {
    await writeIfChanged(bundle.out, await buildPreviewBundle(bundle));
  }
}
