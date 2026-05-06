import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const cssSource = readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8');

/**
 * Regression: tapping the pin button in the mobile thread title row was a no-op
 * on iOS Safari. Two layered causes:
 *
 *   1. `.edge-swipe-zone` (z-index: 1) overlays the leftmost 2.5rem of every
 *      `.mobile-swipe-pane` to bypass iframes that would otherwise capture
 *      touches. The pin button sits at the row's left edge — well inside that
 *      strip. The title row has its own z-index: 2 but lives inside
 *      `.thread-content`, which has `transform: translateZ(0)` (creates a
 *      stacking context with effective z-index 0). The title row's z-index is
 *      trapped inside, so the swipe zone wins hit-testing in iOS Safari.
 *      Fix: give `.thread-content` z-index: 2 to escape the trap.
 *
 *   2. While the prompt textarea is focused, CSS sets pointer-events:none on
 *      `.mobile-thread-title-row` to block stray taps. The pin button needs
 *      pointer-events:auto to remain interactive — pin/unpin is an intentional
 *      in-place action on the current thread, not a navigation.
 */
describe('Mobile pin button — title row tappability', () => {
  it('elevates .thread-content above .edge-swipe-zone via z-index', () => {
    expect(cssSource).toMatch(
      /\.mobile-swipe-pane\s+\.thread-content\s*\{[^}]*z-index:\s*2\s*;/,
    );
  });

  it('keeps the keyboard-active block on .mobile-thread-title-row', () => {
    expect(cssSource).toMatch(
      /:root\[data-keyboard-active\]\s+\.mobile-thread-title-row\s*\{\s*pointer-events:\s*none/,
    );
  });

  it('re-enables pointer-events on .icon-btn inside .mobile-thread-title-row when keyboard is active', () => {
    expect(cssSource).toMatch(
      /:root\[data-keyboard-active\]\s+\.mobile-thread-title-row\s+\.icon-btn\s*\{\s*pointer-events:\s*auto/,
    );
  });
});
