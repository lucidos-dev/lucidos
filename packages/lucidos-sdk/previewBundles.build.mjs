import { build } from 'esbuild';
import { writeIfChanged } from './writeIfChanged.mjs';

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

export async function buildPreviewBundle(bundle, entryPath = bundle.entry) {
  const result = await build({ ...previewBundleBuildOptions(bundle, entryPath), write: false });
  return result.outputFiles[0].text;
}

export async function writePreviewBundles() {
  for (const bundle of PREVIEW_BUNDLES) {
    await writeIfChanged(bundle.out, await buildPreviewBundle(bundle));
  }
}
