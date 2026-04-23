import { build } from 'esbuild';

await build({
  entryPoints: ['src/browser.ts'],
  bundle: true,
  format: 'iife',
  globalName: '__cognosSDK',
  outfile: 'dist/sdk.js',
  target: 'es2020',
  minify: false,
  sourcemap: true,
  footer: {
    // Set up window.cognos from the IIFE export
    js: 'if(typeof window!=="undefined")window.cognos=__cognosSDK.cognos;',
  },
});
