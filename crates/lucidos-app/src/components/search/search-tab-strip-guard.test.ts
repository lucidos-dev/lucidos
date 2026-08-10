/**
 * The Search Everywhere category strip pans horizontally, and a horizontal
 * scroller owes two declarations beyond `overflow-x`. Both were missing when
 * the pan first shipped, and the strip came back scrollable up and down with
 * the tabs' bottom padding dragged out of view.
 *
 * `overflow-y: hidden` is not tidiness. Setting one axis to `auto` computes the
 * OTHER axis from `visible` to `auto`, so the strip silently became a vertical
 * scroller too. It has one line of tabs and can never have anything to scroll
 * to, but its content box does not land on a whole pixel at a scaled root:
 * measured on this stylesheet in both WebKit and Chromium, 35.19px of content
 * inside a 35px scrollport at the 112.5% UI scale the report came from. Neither
 * emulator turns that fraction into a draggable scroll (both round it away and
 * report no overflow), which is exactly why the axis is forbidden outright here
 * rather than left to whatever the device decides to do with the remainder.
 *
 * `flex-shrink: 0` covers the other consequence. The strip is a flex item of
 * `.search-everywhere-modal`'s column, a flex item with visible overflow gets
 * an automatic minimum size, and becoming a scroll container drops that minimum
 * to zero, leaving the strip freely shrinkable. `.search-everywhere-results`
 * would absorb no part of a deficit (`flex: 1` gives it a `0%` basis, and
 * shrinkage is weighted by basis), so all of it lands on the strip. Measured on
 * an EQUIVALENT MINIMAL STRIP rather than on this sheet, in both engines: 35px
 * collapsing to 8px, with the tab squashed to 16px and its underline riding up
 * against the label. This sheet's own modal cap did not reproduce that in
 * either engine, so the declaration is a guard against a real exposure rather
 * than the fix for a reproduced failure. It costs nothing and the strip has no
 * business shrinking.
 *
 * A source scan rather than a browser test, for the reason the measurements
 * above give: the failure lives in device rounding an emulator does not
 * reproduce, so what can be pinned is which declaration is written. The repo's
 * CSS gate (`vite build`) parses the sheet without laying it out.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// `rulesTargeting` rather than the `block`/`decl` string helpers beside it:
// those resolve the first TEXTUAL match, and this sheet already carries a
// `@media (max-width: 768px)` block. A vertical scroll re-enabled in there
// would be invisible to a first-match scan while breaking exactly the surface
// the report came from.
import { cssRules, rulesTargeting, type CssRule } from '../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, './SearchEverywhere.css'), 'utf-8');

const STRIP = 'search-everywhere-tabs';
const TAB = 'search-everywhere-tab';

/** Every rule styling the element, media copies and compound selectors alike. */
function rulesFor(className: string): CssRule[] {
  const rules = rulesTargeting(css, className);
  expect(rules.length, `no rule targets .${className}`).toBeGreaterThan(0);
  return rules;
}

/** The value the sheet lands on for `prop`: the last rule in it that sets one. */
function effective(className: string, prop: string): string | undefined {
  let value: string | undefined;
  for (const rule of rulesFor(className)) {
    const v = rule.props.get(prop);
    if (v !== undefined) value = v;
  }
  return value;
}

/** No rule in the sheet aimed at this element may set `prop` to anything else. */
function neverOverridden(className: string, prop: string, allowed: string): void {
  for (const rule of rulesFor(className)) {
    const v = rule.props.get(prop);
    const where = rule.atRules || 'top level';
    if (v !== undefined) expect(v, `${rule.selector} { ${prop} } under ${where}`).toBe(allowed);
  }
}

describe('the Search Everywhere tab strip pans on one axis only', () => {
  it('scrolls horizontally', () => {
    expect(effective(STRIP, 'overflow-x')).toBe('auto');
  });

  it('never scrolls vertically, however the rule is reached', () => {
    expect(effective(STRIP, 'overflow-y')).toBe('hidden');
    neverOverridden(STRIP, 'overflow-y', 'hidden');
    // `overflow` resets both axes at once and is the shape a scan for the
    // longhand alone would sail past.
    neverOverridden(STRIP, 'overflow', 'hidden');
  });

  it('never absorbs the modal column\'s shrink, however the rule is reached', () => {
    // Load-bearing precisely BECAUSE the strip is a scroll container: that is
    // what took away the automatic minimum size it used to stand on.
    expect(effective(STRIP, 'flex-shrink')).toBe('0');
    neverOverridden(STRIP, 'flex-shrink', '0');
    neverOverridden(STRIP, 'flex', '0 0 auto');
  });

  it('keeps the tabs at their natural width', () => {
    expect(effective(TAB, 'flex-shrink')).toBe('0');
    expect(effective(TAB, 'white-space')).toBe('nowrap');
    neverOverridden(TAB, 'flex-shrink', '0');
  });
});

/**
 * The active category and a keyboard-focused one are the same plain line under
 * the label, differing only in colour, so the only thing keeping a focused tab
 * distinguishable is which of the two `::after` rules the cascade lands on.
 * They carry EQUAL specificity, so that is decided by source order alone, and
 * getting it backwards breaks exactly ONE of the seven tabs: the focus line
 * still shows on the other six, so the bug hides behind them. The one it breaks
 * is the active tab, which already wears a line of its own and would simply
 * keep it, and which is also the tab a user Tabbing into the strip lands on
 * first. Nothing else in the gate reads a cascade, and no test renders this
 * strip.
 *
 * `rulesTargeting` deliberately drops pseudo-element rules (it answers "what
 * styles this BOX"), so this reads the sheet's rule list directly.
 */
describe('the Search Everywhere tab strip always shows where the keyboard is', () => {
  const ACTIVE_LINE = '.search-everywhere-tab.active::after';
  const FOCUS_LINE = '.search-everywhere-tab:focus-visible::after';

  const rules = cssRules(css);

  function indicator(selector: string): { at: number; rule: CssRule } {
    const at = rules.findIndex(r => r.selector === selector);
    expect(at, `no rule for ${selector}`).toBeGreaterThanOrEqual(0);
    return { at, rule: rules[at] };
  }

  it('resolves the focus line last, so a focused active tab still moves', () => {
    const active = indicator(ACTIVE_LINE);
    const focus = indicator(FOCUS_LINE);
    // Both at top level: an @media copy of either would reorder the cascade
    // somewhere this comparison cannot see.
    expect(active.rule.atRules, `${ACTIVE_LINE} is nested`).toBe('');
    expect(focus.rule.atRules, `${FOCUS_LINE} is nested`).toBe('');
    expect(focus.at, `${FOCUS_LINE} must be written after ${ACTIVE_LINE}`)
      .toBeGreaterThan(active.at);
  });

  it('paints the two lines in different colours', () => {
    const activeColor = indicator(ACTIVE_LINE).rule.props.get('background');
    const focusColor = indicator(FOCUS_LINE).rule.props.get('background');
    expect(activeColor, `${ACTIVE_LINE} sets no background`).toBeTruthy();
    expect(focusColor, `${FOCUS_LINE} sets no background`).toBeTruthy();
    expect(focusColor).not.toBe(activeColor);
  });
});
