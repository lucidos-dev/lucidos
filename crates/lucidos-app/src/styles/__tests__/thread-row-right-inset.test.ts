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

import { cssRules, selectorList } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(resolve(here, '../drawer.css'), 'utf8');
const RULES = cssRules(CSS);

/** The one rule whose selector list is exactly `sel`, in that media context,
 *  optionally narrowed to those declaring `prop`. */
function ruleFor(sel: string, atRules: string, prop?: string) {
  const found = RULES.filter(r => r.selector === sel && r.atRules === atRules
    && (!prop || r.props.has(prop)));
  expect(found, `no rule "${sel}" under "${atRules || 'top level'}"`).toHaveLength(1);
  return found[0];
}

const MOBILE = '@media (max-width: 768px)';
const VAR = '--thread-row-pad-right';

describe('the thread row right inset', () => {
  it('is declared once on the drawer, as a var the row pads by', () => {
    // On the drawer rather than the row, so a section header reads it too.
    expect(ruleFor('.thread-drawer', '').props.get(VAR)).toBe('0.5rem');
    expect(ruleFor('.thread-row', '').props.get('padding-right')).toBe(`var(${VAR})`);
    expect(RULES.filter(r => r.props.has(VAR)).map(r => r.selector))
      .toEqual(['.thread-drawer', '.thread-drawer']);
  });

  it('is what both dividers stop at, rather than a second copy', () => {
    const rowDivider = ruleFor('.thread-drawer .thread-row::after', '');
    expect(rowDivider.props.get('right')).toBe(`var(${VAR})`);
    // The collapsed divider and the line that carries it share one rule, so
    // the carried line lands on the divider exactly.
    const shared = RULES.filter(r => r.props.has('right')
      && selectorList(r.selector).includes('.thread-drawer .list-section-title-collapsible.collapsed::after'));
    expect(shared).toHaveLength(1);
    expect(selectorList(shared[0].selector)).toContain('.thread-drawer .flip-disclosure-hairline');
    expect(shared[0].props.get('right')).toBe(`var(${VAR})`);
  });

  it('grows on mobile, so the pin does not inherit the vacated corner', () => {
    // --space-lg is 1rem, twice the desktop 0.5rem. Asserting the token, not a
    // length: the point is that it joins the app's shared gutter.
    expect(ruleFor('.thread-drawer', MOBILE, VAR).props.get(VAR)).toBe('var(--space-lg)');
  });

  it('is the only thing its mobile rule moves', () => {
    expect([...ruleFor('.thread-drawer', MOBILE, VAR).props.keys()]).toEqual([VAR]);
  });
});
