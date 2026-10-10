/**
 * A dropdown's hidden option labels measure width, never height.
 *
 * `.dropdown-sizer` stacks every option label in one grid cell. The trigger is
 * then as wide as the widest of them, and never resizes on a pick. A grid cell
 * is as TALL as its tallest item too, and that is the trap. A label the UI font
 * cannot draw falls back to a font with taller metrics, and raises the whole
 * control.
 *
 * The compose destination picker draws one: its "Register a repository" row
 * opens with a fullwidth plus, which Fira Code has no glyph for. So the picker
 * stood taller than the coding-agent chip beside it. Measured in Chromium
 * against these sheets: 34px beside 31px before the fix, 31px beside 31px
 * after, with the trigger's width unchanged either way.
 *
 * A source scan, because what it pins is a pair of declarations. Only a browser
 * lays out the fallback glyph they defend against.
 */
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

/** Every rule in every sheet that styles a child of `.dropdown-sizer`. */
function sizerChildRules(): CssRule[] {
  const out: CssRule[] = [];
  for (const path of styleSheetPaths(stylesRoot)) {
    for (const rule of cssRules(readFileSync(path, 'utf-8'))) {
      if (selectorList(rule.selector).some(one => /\.dropdown-sizer\s*>/.test(one))) out.push(rule);
    }
  }
  return out;
}

/** Rules that reach the hidden rows alone, leaving the shown one out. */
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
