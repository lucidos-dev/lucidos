/**
 * An Ongoing tile that empties dims in the same frame its count reads 0.
 *
 * A fade on the tile's opacity lifts it onto a compositing layer while the
 * fade runs, and WebKit snaps that layer to whole pixels. The tile sits at a
 * fractional y, so it hopped as the fade began and again as it ended. The 0 also
 * painted at full strength before the dim caught up. jsdom paints nothing, so
 * this scan is the guard.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { cssRules, selectorList } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const drawer = cssRules(readFileSync(resolve(here, '..', 'drawer.css'), 'utf-8'));
/** Rules styling the tile box itself, in any state, not the parts inside it. */
const TILE_BOX = /\.ongoing-tile(-empty|-selected|-highlighted)?(?![-\w])/;
const tileRules = drawer.filter(r => selectorList(r.selector).some(s => TILE_BOX.test(s.trim().split(/\s+/).pop()!)));

describe('an emptied Ongoing tile dims with its count', () => {
  it('the empty tile dims its content, never its fill', () => {
    const content = drawer.find(r => r.selector === '.ongoing-tile-empty:not(.ongoing-tile-selected) > *');
    expect(content?.props.get('opacity')).toBe('var(--drawer-dim-opacity)');
    // A dimmed fill all but vanishes on themes whose tile and strip are close.
    // The unseen halo is a pseudo-element, so its opacity is not the fill's.
    const fillRules = tileRules.filter(r => !r.selector.includes('::'));
    expect(fillRules.filter(r => r.props.has('opacity')).map(r => r.selector)).toEqual([]);
  });

  it('no rule on the tile fades its opacity', () => {
    // Guard the guard: the base rule carries the hover fill's transition.
    expect(tileRules.some(r => r.props.has('transition'))).toBe(true);
    for (const r of tileRules) {
      for (const prop of ['transition', 'transition-property']) {
        expect(r.props.get(prop) ?? '', `${r.selector} { ${prop} }`).not.toMatch(/\b(opacity|all)\b/);
      }
    }
  });
});
