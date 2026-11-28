import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { dirname, resolve } from 'node:path';
import {
  MAX_ID_CHARS,
  MAX_INSTRUCTION_CHARS,
  MAX_LABEL_CHARS,
  MAX_STYLES,
} from './responseStyle';

const HERE = dirname(fileURLToPath(import.meta.url));
const RUST: string = readFileSync(
  resolve(HERE, '../../../../lucidos-core/src/core/response_style.rs'),
  'utf8',
);

function rustUsize(name: string): number {
  const match = RUST.match(new RegExp(`pub const ${name}: usize = ([0-9_]+);`));
  if (!match) throw new Error(`${name} is not declared in core/response_style.rs`);
  return Number(match[1].replace(/_/g, ''));
}

describe('the style-library bounds mirror the engine', () => {
  it('reads a Rust source that actually declares them', () => {
    expect(RUST).toContain('pub const MAX_ID_CHARS');
  });

  it('agrees on every bound', () => {
    expect(MAX_ID_CHARS).toBe(rustUsize('MAX_ID_CHARS'));
    expect(MAX_LABEL_CHARS).toBe(rustUsize('MAX_LABEL_CHARS'));
    expect(MAX_INSTRUCTION_CHARS).toBe(rustUsize('MAX_INSTRUCTION_CHARS'));
    expect(MAX_STYLES).toBe(rustUsize('MAX_STYLES'));
  });
});
