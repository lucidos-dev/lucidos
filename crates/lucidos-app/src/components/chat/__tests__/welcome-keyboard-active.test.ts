import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const cssSource = readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8');

describe('Mobile welcome surface — tappable while composing', () => {
  it('keeps the keyboard-active block on the transcript content', () => {
    expect(cssSource).toMatch(
      /:root\[data-keyboard-active\]\s+\.thread-content\s*>\s*\*\s*\{\s*pointer-events:\s*none/,
    );
  });

  it('re-enables pointer-events on that content in the compose-empty layout', () => {
    expect(cssSource).toMatch(
      /:root\[data-keyboard-active\]\s+\.compose-empty\s+\.thread-content\s*>\s*\*\s*\{\s*pointer-events:\s*auto/,
    );
  });

  it('orders the carve-out after the block so it wins on specificity AND source order', () => {
    const block = cssSource.indexOf(':root[data-keyboard-active] .thread-content > * {');
    const carveOut = cssSource.indexOf(':root[data-keyboard-active] .compose-empty .thread-content > * {');
    expect(block).toBeGreaterThan(-1);
    expect(carveOut).toBeGreaterThan(block);
  });
});
