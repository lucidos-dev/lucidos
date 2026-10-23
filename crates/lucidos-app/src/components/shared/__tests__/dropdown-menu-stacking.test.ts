import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

// :root design tokens live in the base partial; the menu itself in the
// host-chrome partial (it is never served to app iframes).
const baseCss: string = read('../../../styles/global/base.css');
const menuCss: string = read('../../../styles/global/host-components.css');
const dropdownSource: string = read('../Dropdown.tsx');

const TOKENS: Record<string, number> = {};
for (const m of baseCss.matchAll(/--(z-[\w-]+):\s*(\d+)\s*;/g)) {
  TOKENS[m[1]] = parseInt(m[2], 10);
}

/** Resolve a z-index value string: a plain number, var(--token), or
 *  calc(var(--token) ± N). Throws on anything else so an unexpected form fails
 *  loudly instead of silently passing the comparison. */
function resolveZ(expr: string): number {
  const trimmed = expr.trim();
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
 * Regression: an open dropdown menu rendered UNDER the floating header chrome.
 * The composer's destination picker opens upward and, once its option list
 * outgrows the space above the trigger, clamps to the top margin and reaches
 * into the header band, where the header's icons painted straight through it.
 *
 * Two halves, and only both together fix it: the menu must out-rank the header
 * chrome, AND it must be portaled to <body> so that number is reachable. Inside
 * a pane it is not: `.mobile-swipe-pane` is `isolation: isolate` +
 * `translateZ(0)`, a stacking context that caps everything inside it below the
 * header however high the menu asks to be.
 */
describe('dropdown menu stacking (regression: header icons painted over the menu)', () => {
  const menuZ = blockZ(menuCss, '.dropdown-menu');

  it('out-ranks the floating header chrome', () => {
    expect(menuZ).toBeGreaterThan(TOKENS['z-control-panel']);
  });

  it('stays below the modal layer so a modal still covers an open menu', () => {
    expect(menuZ).toBeLessThan(TOKENS['z-modal']);
  });

  it('is portaled to <body>, without which the z-index is capped by the pane', () => {
    // A source scan rather than a render assertion: the portal is a prop on the
    // shared <Overlay>, and dropping it would silently reinstate the bug on
    // mobile (where the pane is a stacking context) while desktop kept working.
    expect(dropdownSource).toMatch(/\n\s*portal\n/);
  });

  it('leaves the header popout token restore unnecessary by construction', () => {
    // The menu is no longer in the `.app-header` subtree, so it inherits the
    // document's dark-on-light defaults. A reintroduced `.app-header
    // .dropdown-menu` rule would mean a menu had stopped portaling.
    const shellCss: string = read('../../../styles/panels/shell.css');
    expect(shellCss).not.toMatch(/\.app-header\s+\.dropdown-menu\s*[,{]/);
  });
});
