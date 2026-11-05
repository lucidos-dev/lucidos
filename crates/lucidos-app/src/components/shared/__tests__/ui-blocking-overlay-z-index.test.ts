import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import { MAX_MODAL_STACK_DEPTH } from '../../../store/overlayStack';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string =>
  readFileSync(resolve(here, '../../../styles', rel), 'utf-8');

// :root design tokens live in the base partial; the overlays themselves are
// defined in modal-overlay.css.
const baseCss = styles('global/base.css');
const modalCss = styles('global/modal-overlay.css');

const TOKENS: Record<string, number> = {};
for (const m of baseCss.matchAll(/--(z-[\w-]+):\s*(\d+)\s*;/g)) {
  TOKENS[m[1]] = parseInt(m[2], 10);
}

/** Resolve a z-index value string: a plain number, var(--token), or
 *  calc(var(--token) ± N). Throws on anything else so an unexpected form fails
 *  loudly instead of silently passing the comparison. */
function resolveZ(expr: string): number {
  const trimmed = expr.trim().replace(/\s*!important$/, '');
  if (/^\d+$/.test(trimmed)) return parseInt(trimmed, 10);
  const varOnly = trimmed.match(/^var\(--(z-[\w-]+)\)$/);
  if (varOnly) return TOKENS[varOnly[1]];
  const calc = trimmed.match(/^calc\(\s*var\(--(z-[\w-]+)\)\s*([+-])\s*(\d+)\s*\)$/);
  if (calc) {
    const base = TOKENS[calc[1]];
    const delta = parseInt(calc[3], 10);
    return calc[2] === '+' ? base + delta : base - delta;
  }
  throw new Error(`Unrecognized z-index expression: "${expr}"`);
}

/** Pull the resolved z-index out of a single-selector rule block. */
function blockZ(css: string, selector: string): number {
  const re = new RegExp(`${selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*\\{([^}]*)\\}`);
  const block = css.match(re);
  expect(block, `selector ${selector} not found`).not.toBeNull();
  const z = block![1].match(/z-index:\s*([^;]+);/);
  expect(z, `z-index not found in ${selector}`).not.toBeNull();
  return resolveZ(z![1]);
}

/**
 * Regression: the UI-blocking overlay blocks the whole UI while the client
 * refreshes (SW swap + reload). Nothing may render above it EXCEPT toasts (so
 * the "Refreshing…" toast stays visible). The drawer's leaving-row copies paint
 * inside the drawer list's stacking context, so they stay under the blocker
 * (`drawer-rows-under-scroll-indicator.test.ts` pins it). Engine restart does
 * not mount this overlay (UiBlockingOverlay.tsx), but it relies on the same
 * z-index ordering.
 */
describe('ui-blocking overlay z-index (only toasts above the blocker)', () => {
  const overlayZ = blockZ(modalCss, '.ui-blocking-overlay');

  it('only the toast layer sits above the blocking overlay', () => {
    expect(TOKENS['z-toast']).toBeGreaterThan(overlayZ);
  });

  /** `--z-modal` is the FLOOR of a band: an open modal adds its overlay-stack
   *  depth, so a confirm raised by a modal is drawn over it. The blocker has to
   *  clear the tallest that band can reach, or a stacked modal punches through
   *  the thing that exists to cover it. */
  it('the whole modal band stays below the blocking overlay', () => {
    expect(TOKENS['z-modal'] + MAX_MODAL_STACK_DEPTH).toBeLessThan(overlayZ);
  });

  it('the tooltip layer is pulled below the overlay while blocked', () => {
    // The JS tooltip is the one --z-tooltip (10000) consumer that outranks the
    // overlay in normal use; :root[data-ui-blocked] makes it step aside. (Two
    // others are gone: the landscape rotate lock, deleted when rotation stopped
    // being an error state, and a pseudo-fullscreen app panel, which moved to
    // --z-app-fullscreen so host modals and toasts can paint over a fullscreen
    // app.)
    expect(modalCss).toMatch(/:root\[data-ui-blocked\]\s+#tooltip\s*\{[^}]*display:\s*none/);
  });

  // The pseudo-fullscreen app panel needs no data-ui-blocked override anymore:
  // it is below the blocker by construction. An override reintroduced here
  // would mean the panel had climbed back above the modal layer.
  it('a pseudo-fullscreen app panel needs no override, being below the blocker', () => {
    expect(TOKENS['z-app-fullscreen']).toBeLessThan(overlayZ);
    expect(modalCss).not.toMatch(/:root\[data-ui-blocked\][^{]*\.app-ui-fullscreen/);
  });
});
