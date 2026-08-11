/**
 * The list of shells that mark a `<shell> -c <script>` wrapper exists three
 * times, and two of them are Rust.
 *
 * `core::WRAPPER_SHELLS` is what the engine uses to build the step-row label it
 * persists on `CodingAgentToolCalled`; `command_guard::GUARD_SHELLS` is the
 * subset the permission guard classifies (a Rust test asserts the containment).
 * The third copy is `WRAPPER_SHELLS` in `store/thread-events/exchange.ts`, which
 * serves the step detail's un-elided value and the fallback label for events
 * stored before descriptions existed. That one is hand-mirrored and nothing
 * compiled it against the Rust side.
 *
 * The two already drifted once, in opposite directions (the label knew `fish`
 * and not `ash`, the guard the reverse), which is what put a `fish -c` payload
 * past one and an `ash -c` payload past the other. Drift here is quiet by
 * construction: a step row would read `ls -la` from the engine's description
 * while the detail beside it read `/bin/mksh -lc 'ls -la'` for the same step,
 * and every suite would still be green.
 *
 * Reading the Rust source from Vitest follows
 * `styles/__tests__/engine-served-css-parses.test.ts`, which gates an
 * engine-embedded stylesheet the same way.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { WRAPPER_SHELLS } from '../thread-events/exchange';

const here = dirname(fileURLToPath(import.meta.url));
/** Repo root, from `crates/lucidos-app/src/store/__tests__/`. */
const REPO_ROOT = resolve(here, '../../../../..');
const ENGINE_CORE = 'crates/lucidos-engine/src/core/mod.rs';

/** The shells named in Rust's `WRAPPER_SHELLS`, read out of the source. */
function rustWrapperShells(): string[] {
  const src: string = readFileSync(resolve(REPO_ROOT, ENGINE_CORE), 'utf8');
  const decl = /const WRAPPER_SHELLS:\s*\[&str;\s*\d+\]\s*=\s*\[([^\]]*)\]/.exec(src);
  expect(
    decl,
    `could not find \`const WRAPPER_SHELLS\` in ${ENGINE_CORE}. If it was renamed or moved, update this mirror rather than deleting it.`,
  ).not.toBeNull();
  return [...decl![1].matchAll(/"([^"]+)"/g)].map(m => m[1]);
}

describe('the wrapper-shell list mirrors the engine', () => {
  it('names exactly the shells `core::WRAPPER_SHELLS` names', () => {
    const rust = rustWrapperShells();
    expect(rust.length).toBeGreaterThan(0);
    // Order carries no meaning in either place (both are membership tests), so
    // compare as sets and report the difference in each direction.
    expect([...WRAPPER_SHELLS].sort()).toEqual([...rust].sort());
  });
});
