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
 * Regression: on mobile, tapping a welcome starter suggestion did nothing once
 * the user had typed a draft.
 *
 * `:root[data-keyboard-active] .thread-content > * { pointer-events: none }`
 * blocks stray taps on the TRANSCRIPT while the on-screen keyboard is up. But in
 * the compose-empty layout `.thread-content` holds the welcome surface instead:
 * the starter-suggestion carousel and the "Don't show this again" pill, which
 * sit directly above the prompt precisely so they stay reachable while
 * composing. `pointer-events` inherits, so the whole surface went dead the
 * moment focus landed in the textarea: the suggestion button was visible and
 * looked enabled, but the hit test fell straight through it.
 *
 * The block targets the transcript's CHILDREN, never `.thread-content` itself.
 * Blocking the scroller took it out of hit-testing entirely, which froze the
 * transcript (a touch has to land on the scroller for the browser to pan it);
 * `styles/__tests__/scroller-hit-target-guard.test.ts` is the guard for that
 * half, and this file owns the carve-out. Both shapes are the same one-line
 * mistake away, so both are pinned.
 *
 * Covered end-to-end by `e2e/welcome.spec.ts` "draft in progress: clicking a
 * suggestion confirms, then overrides the prompt text" on the mobile projects
 * (it passed on desktop chromium — the block is inside `max-width: 768px`).
 * Pinned here too because that spec only runs in the browser e2e suite, and
 * this is a one-line CSS rule that is easy to drop.
 */
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
