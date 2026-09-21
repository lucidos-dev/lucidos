/**
 * Source scans over the one rule that keeps a toggle the size it declares.
 *
 * The reported bug: the three "Resident context" switches under Voice came out
 * three different widths in one column. The narrowest one had its knob hanging
 * past the end of its own pill.
 *
 * A settings row is a flex box, the label beside the switch is free to wrap, and
 * flex distributes shrink over each item's UNWRAPPED width. So a label long
 * enough to wrap took the pill down with it, by an amount that varied with the
 * label. Measured in Chromium against a 200px pane, the three pills came out
 * 23.19px, 36px and 30.75px. The first knob's right edge sat 9.81px past its
 * pill. With `flex-shrink: 0` all three are 36px at every pane width.
 *
 * Nothing renders in a unit test, so those numbers are evidence rather than an
 * assertion. What is asserted here is the shape that produced them, plus the
 * arithmetic that puts the knob on the far inset.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const toggleCss: string = readFileSync(resolve(stylesDir, 'settings', 'toggle.css'), 'utf-8');

const pill = cssRules(toggleCss).find(r => r.selector === '.toggle-switch');
const rem = (v: string | undefined): number => {
  expect(v, 'value is not a rem literal').toMatch(/^[\d.]+rem$/);
  return parseFloat(v!);
};

describe('a toggle is the width it declares', () => {
  it('refuses to shrink, whatever the label beside it does', () => {
    expect(pill, 'no .toggle-switch rule in toggle.css').toBeDefined();
    expect(pill!.props.get('flex-shrink')).toBe('0');
  });

  it('is the only place that decides, so no row has to remember', () => {
    // The models manager carried its own `.model-manager-row .toggle-switch`
    // copy, which is how the defect stayed invisible on the one surface that
    // had already hit it. A second copy means the next flex row is a coin flip.
    //
    // The `flex` shorthand counts. A row setting `flex: 0 1 auto` on the pill
    // resets shrink to 1, at a heavier specificity. That is the reported bug
    // back, and the shorthand carries no `flex-shrink` property to scan for.
    const offenders: string[] = [];
    let seen = 0;
    for (const path of styleSheetPaths(resolve(stylesDir, '..'))) {
      const css: string = readFileSync(path, 'utf-8');
      for (const rule of rulesTargeting(css, 'toggle-switch')) {
        seen++;
        const decides = rule.props.has('flex-shrink') || rule.props.has('flex');
        if (!decides || rule.selector === '.toggle-switch') continue;
        offenders.push(`${path.split('/src/')[1]}: ${rule.selector}`);
      }
    }
    // A FLOOR, so a sweep that matched nothing cannot read as a clean one. One
    // rule takes the pill as its subject today, the base rule above.
    expect(seen, 'the sweep found no .toggle-switch rules at all').toBeGreaterThanOrEqual(1);
    expect(offenders).toEqual([]);
  });
});

describe('the knob travels exactly the width of the pill', () => {
  const travel = cssRules(toggleCss)
    .find(r => r.selector === '.toggle-switch input:checked + .toggle-slider::before');

  it('derives the travel from the pill, rather than restating it', () => {
    // A literal here is right until any one of the three values moves. The knob
    // then stops short of the far end, or hangs over it, silently.
    expect(travel, 'no checked-knob rule in toggle.css').toBeDefined();
    expect(travel!.props.get('transform'))
      .toBe('translateX(calc(var(--toggle-width) - var(--toggle-knob) - 2 * var(--toggle-inset)))');
  });

  it('is declared wide enough for the knob to have somewhere to go', () => {
    // The one thing the string match above cannot see: whether the three
    // NUMBERS are consistent. A knob wider than the pill less its two insets
    // makes the travel negative, and the knob then moves left when switched on.
    const width = rem(pill!.props.get('--toggle-width'));
    const knob = rem(pill!.props.get('--toggle-knob'));
    const inset = rem(pill!.props.get('--toggle-inset'));
    expect(width - knob - 2 * inset, 'the knob has nowhere to travel')
      .toBeGreaterThan(0);
  });

  it('derives the pill height too, so the knob cannot drift off centre', () => {
    // The other axis, and the one easy to leave as a coincidence. The knob is
    // seated on `bottom: var(--toggle-inset)`. So a height stated as a literal
    // stops centring it the moment the knob or the inset changes.
    expect(pill!.props.get('height'))
      .toBe('calc(var(--toggle-knob) + 2 * var(--toggle-inset))');
  });

  it('sizes and seats the knob from those same three values', () => {
    const knob = cssRules(toggleCss).find(r => r.selector === '.toggle-slider::before');
    expect(knob?.props.get('width')).toBe('var(--toggle-knob)');
    expect(knob?.props.get('height')).toBe('var(--toggle-knob)');
    expect(knob?.props.get('left')).toBe('var(--toggle-inset)');
    expect(knob?.props.get('bottom')).toBe('var(--toggle-inset)');
  });
});
