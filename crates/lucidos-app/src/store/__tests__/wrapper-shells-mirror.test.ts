/**
 * The list of shells that mark a `<shell> -c` wrapper exists in Rust twice and
 * in TypeScript once by hand. This file pins the hand-written copy.
 *
 * `core::WRAPPER_SHELLS` is the label list, mirrored by `WRAPPER_SHELLS` in
 * `store/thread-events/exchange.ts`. The label pair drifted once, in opposite
 * directions. `command_guard::GUARD_SHELLS`, the list the permission guard
 * descends into, reaches the card through codegen, so it needs no pin. A Rust
 * test asserts the containment between the two Rust lists.
 *
 * Reading the Rust source from Vitest follows
 * `styles/__tests__/engine-served-css-parses.test.ts`.
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

/** The shells a Rust `[&str; N]` constant names, read out of the source. */
function rustShells(file: string, name: string): string[] {
  const src: string = readFileSync(resolve(REPO_ROOT, file), 'utf8');
  const pattern = new RegExp(`const ${name}:\\s*\\[&str;\\s*\\d+\\]\\s*=\\s*\\[([^\\]]*)\\]`);
  const decl = pattern.exec(src);
  expect(
    decl,
    `could not find \`const ${name}\` in ${file}. If it was renamed or moved, update this mirror rather than deleting it.`,
  ).not.toBeNull();
  const shells = [...decl![1].matchAll(/"([^"]+)"/g)].map(m => m[1]);
  expect(shells.length, `\`${name}\` in ${file} parsed as empty`).toBeGreaterThan(0);
  return shells;
}

describe('the wrapper-shell list mirrors the engine', () => {
  it('names exactly the shells `core::WRAPPER_SHELLS` names', () => {
    const rust = rustShells(ENGINE_CORE, 'WRAPPER_SHELLS');
    // Order carries no meaning in either place (both are membership tests), so
    // compare as sets and report the difference in each direction.
    expect([...WRAPPER_SHELLS].sort()).toEqual([...rust].sort());
  });
});

