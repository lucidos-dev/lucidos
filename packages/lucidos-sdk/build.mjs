import { build } from 'esbuild';
import { buildAllBootBundles } from './appearanceBoot.build.mjs';
import { buildSseWorkerBundle } from './sseWorker.build.mjs';

// The appearance FOUC bundles. Unlike `dist/sdk.js` below these are CHECKED IN,
// because the engine `include_str!`s one of them. See the build module's header.
await buildAllBootBundles();

// The shared SSE worker, checked in for the same reason. See its build module.
await buildSseWorkerBundle();

await build({
  entryPoints: ['src/browser.ts'],
  bundle: true,
  format: 'iife',
  globalName: '__lucidosSDK',
  outfile: 'dist/sdk.js',
  target: 'es2020',
  minify: false,
  sourcemap: true,
  footer: {
    // Set up window.lucidos from the IIFE export
    js: 'if(typeof window!=="undefined")window.lucidos=__lucidosSDK.lucidos;',
  },
});
