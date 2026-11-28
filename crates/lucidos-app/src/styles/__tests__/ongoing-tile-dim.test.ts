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
const TILE_BOX = /\.ongoing-tile(-empty|-selected|-highlighted)?(?![-\w])/;
const tileRules = drawer.filter(r => selectorList(r.selector).some(s => TILE_BOX.test(s.trim().split(/\s+/).pop()!)));

describe('an emptied Ongoing tile dims with its count', () => {
  it('the empty tile dims its content, never its fill', () => {
    const content = drawer.find(r => r.selector === '.ongoing-tile-empty:not(.ongoing-tile-selected) > *');
    expect(content?.props.get('opacity')).toBe('var(--drawer-dim-opacity)');
    const fillRules = tileRules.filter(r => !r.selector.includes('::'));
    expect(fillRules.filter(r => r.props.has('opacity')).map(r => r.selector)).toEqual([]);
  });

  it('no rule on the tile fades its opacity', () => {
    expect(tileRules.some(r => r.props.has('transition'))).toBe(true);
    for (const r of tileRules) {
      for (const prop of ['transition', 'transition-property']) {
        expect(r.props.get(prop) ?? '', `${r.selector} { ${prop} }`).not.toMatch(/\b(opacity|all)\b/);
      }
    }
  });
});
