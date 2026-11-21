/**
 * The SDK still reads historical category grants during migration.
 * Pin its hand-written union to the Rust wire values.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
/** Repo root, from `crates/lucidos-app/src/components/triggers/__tests__/`. */
const REPO_ROOT = resolve(here, '../../../../../..');
const COMMAND_GUARD = 'crates/lucidos-core/src/engine/thread_events/permission/side_effect.rs';
const SDK_TRIGGERS = 'packages/lucidos-sdk/src/triggers.ts';

function source(file: string): string {
  return readFileSync(resolve(REPO_ROOT, file), 'utf8');
}

/** Where a declaration starts, with a message naming the mirror when it moves. */
function declarationAt(src: string, file: string, opener: string): number {
  const start = src.indexOf(opener);
  expect(
    start,
    `could not find \`${opener}\` in ${file}. If it was renamed or moved, update this mirror rather than deleting it.`,
  ).toBeGreaterThan(-1);
  return start;
}

/** PascalCase, as the serde rename writes it on the wire. */
function snake(name: string): string {
  return name.replace(/(?<!^)([A-Z])/g, '_$1').toLowerCase();
}

/**
 * The enum's variants with a wire form. A `#[serde(skip)]` variant has none:
 * historical grants cannot carry it.
 */
function rustValues(): string[] {
  const src = source(COMMAND_GUARD);
  const start = declarationAt(src, COMMAND_GUARD, 'pub enum SideEffectCategory {');
  // The rename attribute is what makes `snake` above the right translation.
  // Drop it and every value becomes PascalCase, which set equality against a
  // snake_case union reports as five renames rather than one.
  expect(src.slice(Math.max(0, start - 400), start)).toContain('rename_all = "snake_case"');
  const body = src
    .slice(start, src.indexOf('\n}', start))
    .split('\n')
    .filter((line) => !line.trim().startsWith('//'))
    .join('\n');
  return [...body.matchAll(/^(\s{4}#\[serde\(skip\)\]\n)?\s{4}([A-Z][A-Za-z0-9]*)\s*,/gm)]
    .filter((m) => m[1] === undefined)
    .map((m) => snake(m[2]));
}

/** The members of an `export type SideEffectCategory = 'a' | 'b'` union. */
function unionMembers(file: string): string[] {
  const src = source(file);
  const start = declarationAt(src, file, 'export type SideEffectCategory =');
  const body = src.slice(start, src.indexOf(';', start));
  return [...body.matchAll(/'([^']+)'/g)].map((m) => m[1]);
}

describe('legacy trigger category grants', () => {
  it('keeps the SDK read type aligned with the Rust wire enum', () => {
    const wireValues = rustValues().sort();
    expect(wireValues.length).toBeGreaterThan(0);
    expect(unionMembers(SDK_TRIGGERS).sort()).toEqual(wireValues);
  });
});
