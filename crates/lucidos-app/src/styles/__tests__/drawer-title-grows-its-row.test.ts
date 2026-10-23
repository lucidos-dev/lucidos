/**
 * A heavier drawer title wraps sooner, so the title and its row must grow with
 * it. Nothing may pin their height or force one line. A theme sets the weight
 * through the `drawer-title` part, so this guard keeps room for the widest one.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { block, decl, rulesTargeting } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const drawerCss = readFileSync(resolve(here, '../drawer.css'), 'utf8');

const CLAMPS = ['height', 'max-height'];

describe('a drawer title grows its row', () => {
  for (const cls of ['thread-row-title', 'thread-row-title-row']) {
    it(`.${cls} sets no height cap and never forces one line`, () => {
      const rules = rulesTargeting(drawerCss, cls);
      expect(rules.length).toBeGreaterThan(0);
      for (const rule of rules) {
        for (const prop of CLAMPS) {
          expect(rule.props.has(prop), `${rule.selector} sets ${prop}`).toBe(false);
        }
        expect(rule.props.get('white-space'), rule.selector).not.toBe('nowrap');
      }
    });
  }

  it('reads its weight from the drawer-title part, falling back to 400', () => {
    expect(decl(block(drawerCss, '.thread-row-title {'), 'font-weight'))
      .toBe('var(--part-drawer-title-font-weight, 400)');
  });
});
