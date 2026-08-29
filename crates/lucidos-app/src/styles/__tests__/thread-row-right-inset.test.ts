/**
 * The mobile thread row's right inset, and the divider that has to share it.
 *
 * On mobile the row's ⋯ is gone and a long press opens the menu instead
 * (`useRowActionsGesture`). That leaves the pin as the row's only control. Let
 * it slide into the vacated slot and it lands back in the corner. So the mobile
 * inset is larger than the desktop one.
 *
 * The two values used to be hand-synced copies: `padding-right` on the row and
 * `right` on the bottom divider, each written `0.5rem`. Moving one and not the
 * other overruns the content column by exactly the difference, which is what
 * `--thread-row-pad-right` now prevents. Pinned here because a var nobody reads
 * is the same bug wearing a better name.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(resolve(here, '../drawer.css'), 'utf8');
const RULES = cssRules(CSS);

/** The one rule whose selector list is exactly `sel`, in that media context. */
function ruleFor(sel: string, atRules: string) {
  const found = RULES.filter(r => r.selector === sel && r.atRules === atRules);
  expect(found, `no rule "${sel}" under "${atRules || 'top level'}"`).toHaveLength(1);
  return found[0];
}

const MOBILE = '@media (max-width: 768px)';

describe('the thread row right inset', () => {
  it('is declared once, as a var the row pads by', () => {
    const row = ruleFor('.thread-row', '');
    expect(row.props.get('--thread-row-pad-right')).toBe('0.5rem');
    expect(row.props.get('padding-right')).toBe('var(--thread-row-pad-right)');
  });

  it('is what the bottom divider stops at, rather than a second copy', () => {
    const divider = ruleFor('.thread-drawer .thread-row::after', '');
    expect(divider.props.get('right')).toBe('var(--thread-row-pad-right)');
  });

  it('grows on mobile, so the pin does not inherit the vacated corner', () => {
    const mobileRow = ruleFor('.thread-row', MOBILE);
    // --space-lg is 1rem, twice the desktop 0.5rem. Asserting the token, not a
    // length: the point is that it joins the app's shared gutter.
    expect(mobileRow.props.get('--thread-row-pad-right')).toBe('var(--space-lg)');
  });

  it('is the only thing the mobile rule moves', () => {
    const mobileRow = ruleFor('.thread-row', MOBILE);
    expect([...mobileRow.props.keys()]).toEqual(['--thread-row-pad-right']);
  });
});
