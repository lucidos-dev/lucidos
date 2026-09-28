/**
 * A drawer row's chips share one bottom edge, also when the name chip wraps.
 *
 * A long trigger name wraps inside its chip, so that chip grows taller than
 * the type tag beside it. Last-baseline alignment puts the short tag and the
 * date on the wrapped chip's last line. That is level with the sub-thread
 * link, and the tall chip grows upward only.
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
  // A family row carries its chips in a box on the sub-thread line instead,
  // so that box needs the same alignment.
  it.each(['.thread-row-meta', '.thread-row-family-chips'])(
    '%s aligns its items on the last baseline',
    (selector) => {
      expect(rule(selector).props.get('align-items')).toBe('last baseline');
    },
  );

  // Aligned against the whole chip box, the link stays at the bottom when the
  // chips wrap to a second line.
  // The link's own rule must say it too: the base `.family-disclosure` rule
  // sets `align-self: flex-start`, which beats the line's `align-items`.
  it('bottom-aligns the sub-thread link with the chip box beside it', () => {
    expect(rule('.thread-row-family-line > .family-disclosure').props.get('align-self')).toBe('flex-end');
  });

  // The skeleton's chip is a bare bar with no text. Left on the baseline, it
  // aligns by its bottom edge and makes the skeleton line taller.
  it('centres the skeleton chip, which has no baseline', () => {
    expect(rule('.thread-row-meta > .sk-bar').props.get('align-self')).toBe('center');
  });
});
