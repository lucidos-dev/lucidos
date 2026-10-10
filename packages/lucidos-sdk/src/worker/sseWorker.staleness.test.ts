/**
 * The committed SSE worker bundle must match what its source builds right now.
 *
 * It is checked in because the engine `include_str!`s it, so `cargo build` must
 * not need npm to have run first. The cost of a committed artifact is that it
 * can go stale, and this is the only thing that would notice.
 *
 * A stale worker is unusually hard to spot. Every document still receives
 * frames, because the PREVIOUS worker relays them perfectly well. Only the
 * edited behaviour goes missing. An unrebuilt change to the pong aggregation
 * would be a silent no-op in the one place deciding whether a push fires.
 *
 * Same contract as `appearanceBoot.staleness.test.ts`, and it rebuilds through
 * the same module `npm run build` calls, so the two cannot disagree about
 * esbuild options.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: a plain .mjs build helper, shared with `npm run build`
import { WORKER_BUNDLE, buildWorkerBundle } from '../../sseWorker.build.mjs';

const here = dirname(fileURLToPath(import.meta.url));
/** The SDK package root, from `packages/lucidos-sdk/src/worker/`. */
const PKG_ROOT = resolve(here, '../..');

describe('the committed SSE worker bundle is current', () => {
  it(`matches its source: ${(WORKER_BUNDLE as { out: string }).out}`, async () => {
    const { entry, out } = WORKER_BUNDLE as { entry: string; out: string };
    // esbuild resolves the entry relative to cwd, which is the app crate when
    // vitest runs. Build from an absolute path so the test works either way.
    const fresh: string = await buildWorkerBundle(resolve(PKG_ROOT, entry));
    const committed = readFileSync(resolve(PKG_ROOT, out), 'utf8');

    expect(
      // Compare the BODY, not the absolute path esbuild writes into the
      // bundle's own comment headers, which differs per checkout.
      normalize(fresh),
      `${out} is stale. Run: cd packages/lucidos-sdk && npm run build\n`
      + 'It is checked in because the engine include_str!s it, so a source edit '
      + 'that was never rebuilt keeps serving the previous worker.',
    ).toBe(normalize(committed));
  });
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
