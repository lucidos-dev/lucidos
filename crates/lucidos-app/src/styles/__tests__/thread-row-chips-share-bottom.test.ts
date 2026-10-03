/**
 * A drawer row's chips share one bottom edge, also when the name chip wraps.
 *
 * A long trigger name wraps inside its chip, so that chip grows taller than
 * the type tag beside it. Last-baseline alignment puts the short tag and the
 * date on the wrapped chip's last line, and the tall chip grows upward only.
 * The sub-thread line below shares one text baseline the same way.
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
const drawerCss = readFileSync(resolve(here, '../drawer.css'), 'utf-8');

/** The rule whose selector list names `selector` as one of its members. */
function rule(selector: string) {
  const found = cssRules(drawerCss).find(r =>
    r.selector.split(',').some(s => s.trim() === selector));
  expect(found, `no rule for ${selector}`).toBeDefined();
  return found!;
}

describe('thread row meta line', () => {
  it('aligns the date and the chips on their last baseline', () => {
    expect(rule('.thread-row-meta').props.get('align-items')).toBe('last baseline');
  });

  // The sub-thread link and the "N archived" label read as one line of text.
  it('puts the sub-thread controls on one text baseline', () => {
    expect(rule('.thread-row-family-line').props.get('align-items')).toBe('last baseline');
  });

  // The base `.family-disclosure` rule sets `align-self: flex-start`, which
  // beats the line's `align-items` and lifts the link off the shared baseline.
  it('lets the sub-thread link follow the line\'s baseline', () => {
    expect(rule('.thread-row-family-line > .family-disclosure').props.get('align-self')).toBe('auto');
  });

  // Centred on the label's lowercase letters, not on its line box.
  it('centres the archived switch on its label', () => {
    expect(rule('.archived-reveal').props.get('display')).toBe('inline-block');
    expect(rule('.archived-reveal-toggle').props.get('vertical-align')).toBe('middle');
  });

  // A line that cannot wrap overflows the row and scrolls the whole drawer
  // sideways. The sub-thread line wraps instead.
  it('wraps the sub-thread line rather than overflowing the row', () => {
    expect(rule('.thread-row-family-line').props.get('flex-wrap')).toBe('wrap');
  });

  // The skeleton's chip is a bare bar with no text. Left on the baseline, it
  // aligns by its bottom edge and makes the skeleton line taller.
  it('centres the skeleton chip, which has no baseline', () => {
    expect(rule('.thread-row-meta > .sk-bar').props.get('align-self')).toBe('center');
  });
});
