import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { WRAPPER_SHELLS } from '../thread-events/tool-descriptions';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../../..');
const ENGINE_CORE = 'crates/lucidos-core/src/core/mod.rs';

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
    expect([...WRAPPER_SHELLS].sort()).toEqual([...rust].sort());
  });
});

