/**
 * The grantable side-effect categories are materialised in four places, and
 * this pins the three TypeScript ones to the Rust enum.
 *
 * Rust owns the wire values (`SideEffectCategory`, serialized snake_case) and
 * the user-facing labels (`label()`). TypeScript restates the values as a union
 * twice, in `store/types.ts` and in the SDK's `triggers.ts`. It restates the
 * value and label pairs once more in this folder's `TriggerDetails.tsx`.
 *
 * Nothing enforced any of it. Add a sixth category in Rust and the trigger
 * editor renders no checkbox for it. A user cannot grant it, the trigger
 * refuses those commands for good, and the page explains nothing.
 *
 * Reading the Rust source from Vitest follows
 * `store/__tests__/wrapper-shells-mirror.test.ts`.
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
const COMMAND_GUARD = 'crates/lucidos-engine/src/engine/command_guard.rs';
const TRIGGER_DETAILS = 'crates/lucidos-app/src/components/triggers/TriggerDetails.tsx';
const STORE_TYPES = 'crates/lucidos-app/src/store/types.ts';
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

/** The enum's variants, in wire form. */
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
  return [...body.matchAll(/^\s{4}([A-Z][A-Za-z0-9]*)\s*,/gm)].map((m) => snake(m[1]));
}

/** Each variant's user-facing label, read off the `label()` match arms. */
function rustLabels(): [string, string][] {
  const src = source(COMMAND_GUARD);
  const start = declarationAt(src, COMMAND_GUARD, "pub fn label(&self) -> &'static str {");
  const body = src.slice(start, src.indexOf('\n    }', start));
  return [...body.matchAll(/Self::([A-Za-z0-9]+)\s*=>\s*"([^"]+)"/g)].map((m) => [
    snake(m[1]),
    m[2],
  ]);
}

/** The value and label pairs the trigger editor offers as checkboxes. */
function editorChoices(): [string, string][] {
  const src = source(TRIGGER_DETAILS);
  const opener = 'const SIDE_EFFECT_CATEGORIES: { value: SideEffectCategory; label: string }[] = [';
  const start = declarationAt(src, TRIGGER_DETAILS, opener);
  const body = src.slice(start, src.indexOf('];', start));
  return [...body.matchAll(/\{\s*value:\s*'([^']+)',\s*label:\s*'([^']+)'\s*\}/g)].map((m) => [
    m[1],
    m[2],
  ]);
}

/** The members of an `export type SideEffectCategory = 'a' | 'b'` union. */
function unionMembers(file: string): string[] {
  const src = source(file);
  const start = declarationAt(src, file, 'export type SideEffectCategory =');
  const body = src.slice(start, src.indexOf(';', start));
  return [...body.matchAll(/'([^']+)'/g)].map((m) => m[1]);
}

describe('every copy of the side-effect categories names the same five', () => {
  it('parses a non-empty list from each of the four sources', () => {
    // A regex that quietly matches nothing reports green for ever, so each
    // parser is checked before the comparisons that rest on it.
    expect(rustValues().length).toBeGreaterThan(0);
    expect(editorChoices().length).toBeGreaterThan(0);
    expect(unionMembers(STORE_TYPES).length).toBeGreaterThan(0);
    expect(unionMembers(SDK_TRIGGERS).length).toBeGreaterThan(0);
  });

  it.each([
    ['the store union', STORE_TYPES],
    ['the SDK union', SDK_TRIGGERS],
  ])('%s carries exactly the enum variants', (_label, file) => {
    expect(unionMembers(file).sort()).toEqual([...rustValues()].sort());
  });

  it('offers a checkbox for every category and no other', () => {
    // The reason this matters most: a category with no checkbox cannot be
    // granted, so the trigger refuses those commands with nothing on the page
    // saying why.
    expect(editorChoices().map(([value]) => value).sort()).toEqual([...rustValues()].sort());
  });
});

describe('a checkbox reads what the engine would call it', () => {
  it('labels each category exactly as `label()` does', () => {
    const rust = rustLabels();
    expect(rust.length, 'no `label()` arms parsed').toBeGreaterThan(0);
    // Sorted pairs, so a swapped label is reported against the value it belongs
    // to rather than as two unrelated differences.
    expect([...editorChoices()].sort()).toEqual([...rust].sort());
  });

  it('labels every variant, leaving none to a fallback', () => {
    expect(rustLabels().map(([value]) => value).sort()).toEqual([...rustValues()].sort());
  });
});
