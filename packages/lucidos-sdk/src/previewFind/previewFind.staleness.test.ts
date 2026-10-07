/**
 * The committed preview finder bundle must match what its source builds now.
 *
 * It is checked in because the host imports it as text. A stale copy would
 * keep the HTML preview matching with the previous matcher while apps and
 * text previews use the new one. Same contract as the SSE worker's staleness
 * test, rebuilt through the module `npm run build` calls.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: a plain .mjs build helper, shared with `npm run build`
import { PREVIEW_FIND_BUNDLE, buildPreviewFindBundle } from '../../previewFind.build.mjs';

const here = dirname(fileURLToPath(import.meta.url));
/** The SDK package root, from `packages/lucidos-sdk/src/previewFind/`. */
const PKG_ROOT = resolve(here, '../..');

describe('the committed preview finder bundle is current', () => {
  it(`matches its source: ${(PREVIEW_FIND_BUNDLE as { out: string }).out}`, async () => {
    const { entry, out } = PREVIEW_FIND_BUNDLE as { entry: string; out: string };
    const fresh: string = await buildPreviewFindBundle(resolve(PKG_ROOT, entry));
    const committed = readFileSync(resolve(PKG_ROOT, out), 'utf8');
    expect(
      normalize(fresh),
      `${out} is stale. Run: cd packages/lucidos-sdk && npm run build`,
    ).toBe(normalize(committed));
  });

  it('can sit inside a script element', () => {
    // The host stamps it into the preview's document text, where `</script`
    // would end the element early.
    const committed = readFileSync(resolve(PKG_ROOT, (PREVIEW_FIND_BUNDLE as { out: string }).out), 'utf8');
    expect(committed.toLowerCase()).not.toContain('</script');
  });
});

/** Drop esbuild's `// <path>` section comments and trailing whitespace. */
function normalize(bundle: string): string {
  return bundle
    .split('\n')
    .filter((line: string) => !/^\s*\/\/ .*\.ts$/.test(line))
    .join('\n')
    .trimEnd();
}
