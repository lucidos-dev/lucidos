/**
 * Build the two appearance boot bundles from one source.
 *
 * They are parser-blocking scripts embedded in two documents that cannot import
 * anything at runtime (the app shell's `<head>`, and every app iframe via the
 * engine's `/api/v1/sdk-prefs.js`), so each has to be a self-contained IIFE.
 * That is a runtime constraint, not a source one: both come from
 * `src/boot/appearanceBoot.ts` and differ only in their entry file.
 *
 * The output is CHECKED IN under `src/generated/`, unlike `dist/sdk.js`. The
 * engine `include_str!`s the iframe bundle, so `cargo build` would otherwise
 * need npm to have run first, and unlike the SDK bundle a boot script has no
 * usable "not built yet" fallback: a missing one is a flash on every cold load.
 * `appearanceBoot.staleness.test.ts` rebuilds and diffs, so a source edit that
 * nobody rebuilt fails the ordinary test run rather than silently shipping the
 * previous script.
 *
 * `es2015` because these run before anything else on whatever the device
 * brought, including an old iOS WKWebView, and there is no bundle to fall back
 * to if the parse fails. `minify` off: both artifacts are read by humans in
 * review, and the shell's copy is read in the page source by anyone debugging a
 * first-paint flash.
 */
import { build } from 'esbuild';

/** Entry file to committed artifact. */
export const BOOT_BUNDLES = [
  { entry: 'src/boot/host.ts', out: 'src/generated/appearance-boot.host.js' },
  { entry: 'src/boot/iframe.ts', out: 'src/generated/appearance-boot.iframe.js' },
];

/** Shared config, so the staleness test builds exactly what `npm run build`
 *  wrote. Any drift between the two would make the check meaningless. */
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
  const { writeFile, mkdir } = await import('node:fs/promises');
  await mkdir('src/generated', { recursive: true });
  for (const { entry, out } of BOOT_BUNDLES) {
    await writeFile(out, await buildBootBundle(entry), 'utf8');
  }
}
