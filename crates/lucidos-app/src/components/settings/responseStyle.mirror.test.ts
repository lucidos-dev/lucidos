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
  STANDARD_ID,
} from './responseStyle';

/** The engine's own copy of the *style library* bounds.
 *
 *  The client refuses a style the engine would refuse, so the two gates must
 *  hold the same numbers. Client-side they only decide whether Save is enabled.
 *  The engine's are what bind. Drift either way is invisible without this.
 *  Raise the engine's and Save stays dead on a paragraph it would take. Lower
 *  it and the client offers a save that comes back as a toast.
 *
 *  Same shape as `utils/imageBytes.mirror.test.ts` and
 *  `voice/frames.mirror.test.ts`: read the Rust source, do not restate it. */
const HERE = dirname(fileURLToPath(import.meta.url));
const RUST: string = readFileSync(
  resolve(HERE, '../../../../lucidos-engine/src/core/response_style.rs'),
  'utf8',
);

function rustUsize(name: string): number {
  const match = RUST.match(new RegExp(`pub const ${name}: usize = ([0-9_]+);`));
  if (!match) throw new Error(`${name} is not declared in core/response_style.rs`);
  return Number(match[1].replace(/_/g, ''));
}

describe('the style-library bounds mirror the engine', () => {
  it('reads a Rust source that actually declares them', () => {
    // Guards the regex itself: a rename would otherwise make every assertion
    // below throw a confusing "not declared" instead of naming the drift.
    expect(RUST).toContain('pub const MAX_ID_CHARS');
  });

  it('agrees on every bound', () => {
    expect(MAX_ID_CHARS).toBe(rustUsize('MAX_ID_CHARS'));
    expect(MAX_LABEL_CHARS).toBe(rustUsize('MAX_LABEL_CHARS'));
    expect(MAX_INSTRUCTION_CHARS).toBe(rustUsize('MAX_INSTRUCTION_CHARS'));
    expect(MAX_STYLES).toBe(rustUsize('MAX_STYLES'));
  });

  it('agrees on the off switch id', () => {
    const match = RUST.match(/pub const STANDARD_ID: &str = "([a-z-]+)";/);
    expect(match?.[1]).toBe(STANDARD_ID);
  });
});
