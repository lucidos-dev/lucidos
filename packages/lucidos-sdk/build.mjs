import { build } from 'esbuild';
import { buildAllBootBundles } from './appearanceBoot.build.mjs';
import { buildSseWorkerBundle } from './sseWorker.build.mjs';
import { writePreviewBundles } from './previewBundles.build.mjs';

// Every output lands in dist/ and is never committed. `scripts/codegen.sh`
// runs this after the Rust-generated TS these bundles import.

// The appearance FOUC bundles. See the build module's header.
await buildAllBootBundles();

// The shared SSE worker. See its build module.
await buildSseWorkerBundle();

// The HTML preview's finder and pane swipe, which the host imports as text.
await writePreviewBundles();

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
