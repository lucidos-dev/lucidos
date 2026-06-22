import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string =>
  readFileSync(resolve(here, '../../../styles', rel), 'utf-8');

// :root design tokens live in the base partial; the overlays themselves are
// defined in modal-overlay.css and drawer.css.
const baseCss = styles('global/base.css');
const modalCss = styles('global/modal-overlay.css');
const drawerCss = styles('drawer.css');

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
 * the "Refreshing…" toast stays visible). Previous bug: drawer thread rows whose
 * status changed flew above the dim blocker because the FLIP animation portal sat
 * at z-index 9999. (Engine restart no longer mounts this overlay — see
 * UiBlockingOverlay.tsx — but the z-index ordering it relies on is unchanged.)
 */
describe('ui-blocking overlay z-index (only toasts above the blocker)', () => {
  const overlayZ = blockZ(modalCss, '.ui-blocking-overlay');
  const flipPortalZ = blockZ(drawerCss, '.flip-portal');

  it('a flying drawer thread (.flip-portal) stays below the blocking overlay', () => {
    expect(flipPortalZ).toBeLessThan(overlayZ);
  });

  it('the flying thread still renders above app chrome so it is never clipped', () => {
    expect(flipPortalZ).toBeGreaterThan(TOKENS['z-control-panel']);
  });

  it('only the toast layer sits above the blocking overlay', () => {
    expect(TOKENS['z-toast']).toBeGreaterThan(overlayZ);
  });

  it('tooltip-layer elements are pulled below the overlay while blocked', () => {
    // The three --z-tooltip (10000) consumers — the JS tooltip, the
    // pseudo-fullscreen app iframe, and the landscape lock — all outrank the
    // overlay in normal use; :root[data-ui-blocked] makes them step aside.
    expect(modalCss).toMatch(/:root\[data-ui-blocked\]\s+#tooltip\s*\{[^}]*display:\s*none/);
    expect(modalCss).toMatch(/:root\[data-ui-blocked\][^{]*\.app-ui-fullscreen/);
    expect(modalCss).toMatch(/:root\[data-ui-blocked\][^{]*\.landscape-lock/);
  });
});
