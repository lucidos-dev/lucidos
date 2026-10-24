/**
 * The HTML preview bundles, built in memory through the module `npm run build`
 * calls.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: a plain .mjs build helper, shared with `npm run build`
import { PREVIEW_BUNDLES, buildPreviewBundle } from '../previewBundles.build.mjs';

interface PreviewBundle { entry: string; out: string; globalName: string }

const here = dirname(fileURLToPath(import.meta.url));
/** The SDK package root, from `packages/lucidos-sdk/src/`. */
const PKG_ROOT = resolve(here, '..');

describe.each(PREVIEW_BUNDLES as PreviewBundle[])('the preview bundle $out', (bundle) => {
  it('can sit inside a script element', async () => {
    // esbuild resolves the entry relative to cwd, which is the app crate when
    // Vitest runs, so build from an absolute path.
    const built: string = await buildPreviewBundle(bundle, resolve(PKG_ROOT, bundle.entry));
    // The host stamps it into the preview's document text, where `</script`
    // would end the element early.
    expect(built.toLowerCase()).not.toContain('</script');
  });
});
