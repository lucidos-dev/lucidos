/**
 * Build the two appearance boot bundles from one source.
 *
 * They are parser-blocking scripts embedded in two documents that cannot import
 * anything at runtime (the app shell's `<head>`, and every app iframe via the
 * engine's `/api/v1/sdk-prefs.js`), so each has to be a self-contained IIFE.
 * That is a runtime constraint, not a source one: both come from
 * `src/boot/appearanceBoot.ts` and differ only in their entry file.
 *
 * The output lands in `dist/`, beside `sdk.js`, and is never committed. The
 * engine reads the iframe bundle at startup, and Vite inlines the host bundle.
 * Every consumer builds the SDK first, through `scripts/codegen.sh`. A boot
 * script has no usable "not built yet" fallback, so a missing one is an error
 * at engine startup, never a silent stub.
 *
 * `es2015` because these run before anything else on whatever the device
 * brought, including an old iOS WKWebView, and there is no bundle to fall back
 * to if the parse fails. `minify` off: both artifacts are read by humans in
 * review, and the shell's copy is read in the page source by anyone debugging a
 * first-paint flash.
 */
import { build } from 'esbuild';
import { writeIfChanged } from './writeIfChanged.mjs';

/** Entry file to build output. */
export const BOOT_BUNDLES = [
  { entry: 'src/boot/host.ts', out: 'dist/appearance-boot.host.js' },
  { entry: 'src/boot/iframe.ts', out: 'dist/appearance-boot.iframe.js' },
];


export function bootBuildOptions(entry) {
  return {
    entryPoints: [entry],
    bundle: true,
    format: 'iife',
    target: 'es2015',
    minify: false,
    sourcemap: false,
    banner: {
      js: '/* GENERATED from packages/lucidos-sdk/src/boot/ by appearanceBoot.build.mjs.\n'
        + '   Do not edit: run `npm run build` in packages/lucidos-sdk. */',
    },
  };
}

/** The bundle text for one entry, without touching disk. */
export async function buildBootBundle(entry) {
  const result = await build({ ...bootBuildOptions(entry), write: false });
  return result.outputFiles[0].text;
}

export async function buildAllBootBundles() {
  for (const { entry, out } of BOOT_BUNDLES) {
    await writeIfChanged(out, await buildBootBundle(entry));
  }
}
