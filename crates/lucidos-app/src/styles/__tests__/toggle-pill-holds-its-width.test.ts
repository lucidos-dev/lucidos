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
const toggleCss: string = readFileSync(resolve(stylesDir, 'global', 'shared-components.css'), 'utf-8');

const pill = cssRules(toggleCss).find(r => r.selector === '.toggle-switch');
const rem = (v: string | undefined): number => {
  expect(v, 'value is not a rem literal').toMatch(/^[\d.]+rem$/);
  return parseFloat(v!);
};

describe('a toggle is the width it declares', () => {
  it('refuses to shrink, whatever the label beside it does', () => {
    expect(pill, 'no .toggle-switch rule in shared-components.css').toBeDefined();
    expect(pill!.props.get('flex-shrink')).toBe('0');
  });

  it('is the only place that decides, so no row has to remember', () => {
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
    expect(seen, 'the sweep found no .toggle-switch rules at all').toBeGreaterThanOrEqual(1);
    expect(offenders).toEqual([]);
  });
});

describe('the knob travels exactly the width of the pill', () => {
  const travel = cssRules(toggleCss)
    .find(r => r.selector === '.toggle-switch input:checked + .toggle-slider::before');

  it('derives the travel from the pill, rather than restating it', () => {
    expect(travel, 'no checked-knob rule in shared-components.css').toBeDefined();
    expect(travel!.props.get('transform'))
      .toBe('translateX(calc(var(--toggle-width) - var(--toggle-knob) - 2 * var(--toggle-inset)))');
  });

  it('is declared wide enough for the knob to have somewhere to go', () => {
    const width = rem(pill!.props.get('--toggle-width'));
    const knob = rem(pill!.props.get('--toggle-knob'));
    const inset = rem(pill!.props.get('--toggle-inset'));
    expect(width - knob - 2 * inset, 'the knob has nowhere to travel')
      .toBeGreaterThan(0);
  });

  it('derives the pill height too, so the knob cannot drift off centre', () => {
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
