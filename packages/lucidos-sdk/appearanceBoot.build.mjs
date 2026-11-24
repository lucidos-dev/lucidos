import { build } from 'esbuild';
import { writeIfChanged } from './writeIfChanged.mjs';

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

export async function buildBootBundle(entry) {
  const result = await build({ ...bootBuildOptions(entry), write: false });
  return result.outputFiles[0].text;
}

export async function buildAllBootBundles() {
  for (const { entry, out } of BOOT_BUNDLES) {
    await writeIfChanged(out, await buildBootBundle(entry));
  }
}
