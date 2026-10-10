/**
 * Source scans over the drawer's "Show / Hide N sub-threads" link.
 *
 * The chevron rotates 90 degrees on
 * expand. In the packaged macOS app it then jumped a little to the right once
 * the rotation finished. WebKit drops the compositing layer a transition runs
 * on and repaints the rotated chevron into the row, snapped to a different
 * sub-pixel spot. A permanent layer removes the hand-off.
 *
 * A source scan because nothing this repo drives reproduces it: the box does
 * not move, and Playwright's WebKit paints it steady.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const drawerCss: string = readFileSync(resolve(here, '../drawer.css'), 'utf-8');

function rule(selector: string): Map<string, string> {
  const found = cssRules(drawerCss).filter(r => r.selector === selector);
  expect(found.length, `expected exactly one \`${selector}\` rule`).toBe(1);
  return found[0].props;
}

describe('the sub-thread chevron keeps one layer through its turn', () => {
  it('rests on an identity rotation with a standing layer', () => {
    const chevron = rule('.family-disclosure svg');
    expect(chevron.get('transform'), 'the rest state must be a value on the same property')
      .toBe('rotate(0deg)');
    expect(chevron.get('will-change'), 'without it the layer is torn down as the turn lands')
      .toBe('transform');
    expect(chevron.get('transition')).toContain('transform');
  });

  it('turns by changing that value, not by adding a new property', () => {
    expect(rule('.family-disclosure[aria-expanded="true"] svg').get('transform'))
      .toBe('rotate(90deg)');
  });
});

describe('the sub-thread link stays on one line', () => {
  // A wide chip squeezed the title column, and the button centred its wrapped
  // text, leaving "threads" alone under "Hide 2 sub-".
  it('never wraps its label, and the mobile rule does not undo that', () => {
    const rules = cssRules(drawerCss).filter(r => r.selector === '.family-disclosure');
    expect(rules[0].props.get('white-space')).toBe('nowrap');
    for (const r of rules.slice(1)) expect(r.props.has('white-space')).toBe(false);
  });
});
