import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { cssRules, selectorList } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const RULES = cssRules(readFileSync(resolve(here, '../drawer.css'), 'utf8'));

function prop(selector: string, name: string): string | undefined {
  return RULES.find(r => selectorList(r.selector).includes(selector) && r.props.has(name))?.props.get(name);
}

describe('the thread drawer list', () => {
  it('holds positioned rows, which is what needs the stacking context', () => {
    expect(prop('.thread-row', 'position')).toBe('relative');
  });

  for (const scroller of ['.thread-drawer-list', '.thread-view-drawing-list']) {
    it(`${scroller} is a stacking context, so it paints its own rows`, () => {
      expect(prop(scroller, 'isolation')).toBe('isolate');
    });
  }
});
