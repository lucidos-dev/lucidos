/**
 * The list of shells that mark a `<shell> -c` wrapper exists four times, two in
 * Rust and two in TypeScript. This file pins both TypeScript copies.
 *
 * `core::WRAPPER_SHELLS` is the label list, mirrored by `WRAPPER_SHELLS` in
 * `store/thread-events/exchange.ts`. `command_guard::GUARD_SHELLS` is the
 * smaller list the permission guard descends into, mirrored by `GUARD_SHELLS`
 * in `components/chat/PermissionCard.tsx`. A Rust test asserts the containment
 * between the two Rust lists.
 *
 * The label pair drifted once, in opposite directions. Guard drift is worse:
 * the card unwraps the wrapper to name a grant, and the engine derives the
 * pattern it STORES from the same unwrap. A shell the engine sees through and
 * the card does not makes the card read "Always allow `Bash(mksh:*)`" while the
 * click persists `Bash(rm:*)`.
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
const COMMAND_GUARD = 'crates/lucidos-engine/src/engine/command_guard.rs';
const PERMISSION_CARD = 'crates/lucidos-app/src/components/chat/PermissionCard.tsx';

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

/** The shells `PermissionCard.tsx`'s `GUARD_SHELLS` names.
 *
 *  Read out of the source because that constant is module-private, and widening
 *  a component's export surface for a test is the wrong trade. */
function cardGuardShells(): string[] {
  const src: string = readFileSync(resolve(REPO_ROOT, PERMISSION_CARD), 'utf8');
  const decl = /const GUARD_SHELLS:\s*ReadonlySet<string>\s*=\s*new Set\(\[([^\]]*)\]\)/.exec(src);
  expect(
    decl,
    `could not find \`const GUARD_SHELLS\` in ${PERMISSION_CARD}. If it was renamed or moved, update this mirror rather than deleting it.`,
  ).not.toBeNull();
  const shells = [...decl![1].matchAll(/'([^']+)'/g)].map(m => m[1]);
  expect(shells.length, 'the card `GUARD_SHELLS` parsed as empty').toBeGreaterThan(0);
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

describe('the permission card sees through what the guard sees through', () => {
  it('names exactly the shells `command_guard::GUARD_SHELLS` names', () => {
    const rust = rustShells(COMMAND_GUARD, 'GUARD_SHELLS');
    expect(cardGuardShells().sort()).toEqual([...rust].sort());
  });

  it('never claims a shell the engine will not unwrap', () => {
    // The card's list is the guard's, so it is also a subset of the label list.
    // A shell only the card knows reads the other failure direction: the card
    // names the inner head while the engine stores the wrapper's.
    const label = new Set(rustShells(ENGINE_CORE, 'WRAPPER_SHELLS'));
    for (const shell of cardGuardShells()) {
      expect(label.has(shell), `${shell} is not in \`core::WRAPPER_SHELLS\``).toBe(true);
    }
  });
});
