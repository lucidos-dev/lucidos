import { build } from 'esbuild';

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
