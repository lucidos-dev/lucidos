/**
 * The committed appearance boot bundles must match what their source builds
 * right now.
 *
 * They are checked in because the engine `include_str!`s the iframe one, so
 * `cargo build` must not need npm to have run first. The cost of a committed
 * artifact is that it can go stale, and this is the only thing that would
 * notice: an edit to `boot/appearanceBoot.ts` or to the appearance contract
 * that nobody rebuilt keeps shipping the PREVIOUS script, so the change is a
 * silent no-op in exactly the place a no-op is hardest to spot, before any
 * module has loaded.
 *
 * Same contract as the generated `thread-lifecycle.ts` (regenerate with a named
 * command, staleness checked in the ordinary test run). It rebuilds through the
 * same module `npm run build` calls, so the two cannot disagree about esbuild
 * options.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: a plain .mjs build helper, shared with `npm run build`
import { BOOT_BUNDLES, buildBootBundle } from '../../appearanceBoot.build.mjs';

const here = dirname(fileURLToPath(import.meta.url));
/** The SDK package root, from `packages/lucidos-sdk/src/boot/`. */
const PKG_ROOT = resolve(here, '../..');

describe('the committed appearance boot bundles are current', () => {
  for (const { entry, out } of BOOT_BUNDLES as Array<{ entry: string; out: string }>) {
    it(`matches its source: ${out}`, async () => {
      // esbuild resolves the entry relative to cwd, which is the app crate when
      // vitest runs. Build from an absolute path so the test works either way.
      const fresh: string = await buildBootBundle(resolve(PKG_ROOT, entry));
      const committed = readFileSync(resolve(PKG_ROOT, out), 'utf8');

      expect(
        // Compare the BODY, not the absolute path esbuild writes into the
        // bundle's own comment headers, which differs per checkout.
        normalize(fresh),
        `${out} is stale. Run: cd packages/lucidos-sdk && npm run build\n`
        + 'It is checked in because the engine include_str!s it, so a source edit '
        + 'that was never rebuilt keeps serving the previous boot script.',
      ).toBe(normalize(committed));
    });
  }
});

/** Drop esbuild's `// <path>` section comments, which carry the entry path as
 *  esbuild resolved it, and collapse trailing whitespace. */
function normalize(bundle: string): string {
  return bundle
    .split('\n')
    .filter((line: string) => !/^\s*\/\/ .*\.ts$/.test(line))
    .join('\n')
    .trimEnd();
}
