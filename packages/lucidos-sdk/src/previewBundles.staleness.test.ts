/**
 * Every committed HTML preview bundle must match what its source builds now.
 *
 * They are checked in because the host imports them as text. A stale copy
 * would keep the HTML preview on the previous code while apps and text
 * previews run the new one. Same contract as the SSE worker's staleness test,
 * rebuilt through the module `npm run build` calls.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: a plain .mjs build helper, shared with `npm run build`
import { PREVIEW_BUNDLES, buildPreviewBundle } from '../previewBundles.build.mjs';

interface PreviewBundle { entry: string; out: string; globalName: string }

const here = dirname(fileURLToPath(import.meta.url));
/** The SDK package root, from `packages/lucidos-sdk/src/`. */
const PKG_ROOT = resolve(here, '..');

describe.each(PREVIEW_BUNDLES as PreviewBundle[])('the committed preview bundle $out', (bundle) => {
  const committed = () => readFileSync(resolve(PKG_ROOT, bundle.out), 'utf8');

  it('matches its source', async () => {
    const fresh: string = await buildPreviewBundle(bundle, resolve(PKG_ROOT, bundle.entry));
    expect(
      normalize(fresh),
      `${bundle.out} is stale. Run: cd packages/lucidos-sdk && npm run build`,
    ).toBe(normalize(committed()));
  });

  it('can sit inside a script element', () => {
    // The host stamps it into the preview's document text, where `</script`
    // would end the element early.
    expect(committed().toLowerCase()).not.toContain('</script');
  });
});

/** Drop esbuild's `// <path>` section comments and trailing whitespace. */
function normalize(text: string): string {
  return text
    .split('\n')
    .filter((line: string) => !/^\s*\/\/ .*\.ts$/.test(line))
    .join('\n')
    .trimEnd();
}
