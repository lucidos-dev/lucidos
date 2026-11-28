import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, selectorList, styleSheetPaths, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '..');

function sizerChildRules(): CssRule[] {
  const out: CssRule[] = [];
  for (const path of styleSheetPaths(stylesRoot)) {
    for (const rule of cssRules(readFileSync(path, 'utf-8'))) {
      if (selectorList(rule.selector).some(one => /\.dropdown-sizer\s*>/.test(one))) out.push(rule);
    }
  }
  return out;
}

function hiddenRowRules(): CssRule[] {
  return sizerChildRules().filter(r =>
    selectorList(r.selector).every(one => one.includes(':not(:first-child)')));
}

describe('a dropdown sizer row adds no height', () => {
  it('holds the hidden labels at zero height, so the tallest cannot raise the trigger', () => {
    const zeroing = hiddenRowRules().filter(r => r.props.get('height') === '0');
    expect(zeroing.length, 'no rule holds the hidden option labels at zero height').toBe(1);
  });

  it('clips them, so their text never reaches a scrolling ancestor', () => {
    const rule = hiddenRowRules().find(r => r.props.get('height') === '0');
    expect(
      rule?.props.get('overflow'),
      'a zero-height row spills its line into whatever scrolls above it',
    ).toBe('hidden');
  });

  it('leaves the shown label its own height, so the trigger still has one', () => {
    const everyChild = sizerChildRules().filter(r =>
      selectorList(r.selector).some(one => /\.dropdown-sizer\s*>\s*\*\s*$/.test(one)));
    expect(everyChild.length, 'no rule styles the sizer rows at all').toBeGreaterThan(0);
    for (const rule of everyChild) {
      expect(rule.props.get('height'), `${rule.selector} sizes the shown label too`).toBeUndefined();
    }
  });
});
