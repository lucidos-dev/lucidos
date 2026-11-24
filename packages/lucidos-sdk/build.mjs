import { build } from 'esbuild';
import { buildAllBootBundles } from './appearanceBoot.build.mjs';
import { buildSseWorkerBundle } from './sseWorker.build.mjs';
import { writePreviewBundles } from './previewBundles.build.mjs';


await buildAllBootBundles();

await buildSseWorkerBundle();

await writePreviewBundles();

await build({
  entryPoints: ['src/browser.ts'],
  bundle: true,
  jsx: 'automatic',
  jsxImportSource: 'preact',
  alias: {
    '@lucidos/motion': './src/motion.ts',
    '@lucidos/appearance': './src/appearance.ts',
    '@lucidos/tap-press': './src/tapPress.ts',
    '@lucidos/geometry': './src/geometry.ts',
  },
  format: 'iife',
  globalName: '__lucidosSDK',
  outfile: 'dist/sdk.js',
  target: 'es2020',
  minify: false,
  sourcemap: true,
  footer: {
    js: 'if(typeof window!=="undefined")window.lucidos=__lucidosSDK.lucidos;',
  },
});
