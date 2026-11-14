/**
 * `.list-row-add-card` and `.settings-nav-row` are worn by `<button>` elements
 * (see components/shared/__tests__/clickable-control-element-guard.test.ts for
 * why they are buttons at all). A button arrives with UA chrome a div never
 * had, so each class has to reset it or the control changes appearance:
 * a grey button face, a hairline border, a shrink-to-fit box, and the UA's
 * centered sans-serif in place of the app's font.
 *
 * One of those resets is subtler than it looks, and is the reason this guard
 * asserts on source ORDER and not just on presence: `font: inherit` resets
 * `font-feature-settings` to initial, handing ligatures back to a user on Fira
 * Code, so it must be followed by an explicit `font-feature-settings: inherit`.
 * Exactly the trap already documented at `.accent-link`.
 *
 * `width: 100%` is pinned as belt and braces rather than as load-bearing: both
 * classes also set `display: flex`, which makes the box block-level and so
 * fills on its own. It is here because a UA button is `inline-block` and that
 * is what either rule would fall back to.
 *
 * The focus rings are pinned too, because a button that is focusable but shows
 * nothing on focus is only half the fix, and their DIRECTIONS differ for a
 * reason the sheets explain: the add card runs edge to edge in Apps, Triggers
 * and Accounts under a clipping `overflow-x: hidden`, so its band is inset.
 *
 * Nothing else in the gate parses CSS (`tsc` skips it; `vite build` only fails
 * on syntax), which is why this is a test rather than a review note.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { rulesTargeting, type CssRule } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));

const SHEETS: Record<string, string> = {
  'list-row-add-card': readFileSync(resolve(here, '../global/shared-components.css'), 'utf8'),
  'settings-nav-row': readFileSync(resolve(here, '../settings/base.css'), 'utf8'),
};

/** The class's own bare top-level rule: the one carrying the reset. Matched by
 *  exact selector, so a `:hover` / `:focus-visible` rule, an `@media` copy, and
 *  the higher-specificity `.settings-section-title.settings-nav-row` (which
 *  legitimately restates the row's font size, weight and colour) are all
 *  excluded. A reset that landed only in one of those would not satisfy this. */
function baseRule(className: string): CssRule {
  const rules = rulesTargeting(SHEETS[className], className).filter(
    r => !r.atRules && r.selector === `.${className}`,
  );
  expect(rules.length, `expected exactly one base rule for .${className}`).toBe(1);
  return rules[0];
}

function focusRule(className: string): CssRule {
  const rules = rulesTargeting(SHEETS[className], className).filter(r =>
    r.selector.includes(':focus-visible'),
  );
  expect(rules.length, `expected exactly one :focus-visible rule for .${className}`).toBe(1);
  return rules[0];
}

/** The resets every one of these buttons needs to stop looking like a button. */
const RESET: Record<string, string> = {
  background: 'none',
  border: 'none',
  width: '100%',
  font: 'inherit',
  'font-feature-settings': 'inherit',
  'text-align': 'left',
};

describe.each(Object.keys(SHEETS))('.%s button reset', className => {
  const rule = baseRule(className);

  it.each(Object.entries(RESET))('resets %s to %s', (prop, value) => {
    expect(rule.props.get(prop)).toBe(value);
  });

  it('re-inherits font-feature-settings AFTER font, which resets it to initial', () => {
    const order = rule.body.split('; ').map(d => d.split(':')[0]);
    expect(order.indexOf('font-feature-settings')).toBeGreaterThan(order.indexOf('font'));
  });
});

/**
 * `color` is the one reset that differs between the two, on purpose, and each
 * direction is a finding a review would otherwise make twice.
 *
 * The nav row is already coloured by `.settings-section-title.settings-nav-row`
 * at higher specificity, so a `color` on the bare rule wins nothing and puts a
 * fourth declaration back into a cascade that 55a8143c4 deliberately collapsed.
 * Nothing colours the add card, so there it is the only thing standing between
 * a bare text node and the UA's `buttontext`.
 */
describe('color reset', () => {
  it('the add card resets it, since nothing else colours the card', () => {
    expect(baseRule('list-row-add-card').props.get('color')).toBe('inherit');
  });

  it('the settings nav row does NOT, since a higher-specificity rule already does', () => {
    expect(baseRule('settings-nav-row').props.has('color')).toBe(false);
    const colouring = rulesTargeting(SHEETS['settings-nav-row'], 'settings-nav-row').filter(
      r => !r.atRules && r.props.has('color') && r.selector !== '.settings-nav-row',
    );
    expect(colouring.map(r => r.selector)).toContain('.settings-section-title.settings-nav-row');
  });
});

describe('focus rings', () => {
  it('the add card draws the shared band INSET, since it can run edge to edge under a clip', () => {
    const rule = focusRule('list-row-add-card');
    expect(rule.props.get('box-shadow')).toBe('inset var(--focus-ring)');
    // The forced-colors fallback (box-shadow is stripped there) has to be
    // pulled inside too, or it is clipped exactly where the shadow would be.
    expect(rule.props.get('outline')).toBe('0.125rem solid transparent');
    expect(rule.props.get('outline-offset')).toBe('-0.125rem');
  });

  it('the settings nav row draws the shared band outward, into the panel gutter', () => {
    const rule = focusRule('settings-nav-row');
    expect(rule.props.get('box-shadow')).toBe('var(--focus-ring)');
    expect(rule.props.get('outline')).toBe('0.125rem solid transparent');
  });
});
