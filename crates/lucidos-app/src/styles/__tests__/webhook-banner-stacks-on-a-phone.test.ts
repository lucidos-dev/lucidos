/**
 * The webhook and agent sandbox bars keep the icon beside their sentence
 * and the actions below on a phone.
 *
 * Photographed on a real iPhone in portrait: the refusal notice wrapped inside
 * roughly 60% of the bar. It ran to ten lines, beside a tall empty gap. The
 * button cannot shrink, so on a narrow viewport it keeps a column the sentence
 * pays for.
 *
 * Four properties, each a property of the RULE rather than of a rendered frame.
 * That is cheaper to assert here than in a browser.
 *
 * 1. ONE grouped rule covers all three bars. They share every other
 *    declaration on purpose, and a second copy is how they drift apart.
 * 2. The sentence shares its row with the icon, and the buttons wrap below it.
 * 3. The breakpoint is the one the sheet already switches its mobile layout at.
 * 4. Desktop keeps its single row, which is the half a mobile fix can break
 *    without anyone on a phone noticing.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { type CssRule, rulesTargeting, selectorList } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const shellCss = readFileSync(resolve(here, '../panels/shell.css'), 'utf-8');

/** The sheet's mobile breakpoint, as `cssRules` reports the at-rule. */
const MOBILE = '@media (--phone-layout)';

/** Every bar the grouped rules cover, in sheet order. */
const BARS = ['ingress-banner', 'refusal-banner', 'sandbox-banner'];

/** The one rule that styles `className` below the mobile breakpoint. Asserting
 *  there is exactly one is half the point: a second copy would be the drift. */
function stackRule(className: string): CssRule {
  const found = rulesTargeting(shellCss, className).filter(r => r.atRules === MOBILE);
  expect(found, `${className} takes ${found.length} mobile rules, not one`).toHaveLength(1);
  return found[0];
}

/** Every rule styling `className` OUTSIDE the mobile breakpoint. */
function desktopRules(className: string): CssRule[] {
  return rulesTargeting(shellCss, className).filter(r => !r.atRules.includes('(--phone-layout)'));
}

describe('the webhook and sandbox bars stack on a phone', () => {
  it('wraps every bar from one rule, so they cannot drift apart', () => {
    const bar = stackRule('refusal-banner');
    expect(selectorList(bar.selector)).toEqual(BARS.map(b => `.${b}`));
    for (const other of BARS) expect(stackRule(other).selector).toBe(bar.selector);
    expect(bar.props.get('flex-wrap')).toBe('wrap');
    // The buttons keep the trailing edge they hold on the desktop row.
    expect(bar.props.get('justify-content')).toBe('flex-end');
  });

  it('reserves room for the leading icon on every bar', () => {
    const text = stackRule('refusal-banner-text');
    expect(selectorList(text.selector)).toEqual(BARS.map(b => `.${b}-text`));
    for (const other of BARS) expect(stackRule(`${other}-text`).selector).toBe(text.selector);
    // A full-width sentence strands the icon on its own right-aligned row.
    // Reserve its fixed width and the shared column gap before wrapping actions.
    expect(text.props.get('flex-basis')).toBe('calc(100% - var(--icon-size-md) - var(--space-md))');
  });

  it('spends a spacing token on the gap it opens', () => {
    // Stacked, the bar grows a vertical gap it never had. A literal here is out
    // of the live style remote's reach, same as on the connection bar.
    expect(stackRule('refusal-banner').props.get('row-gap')).toBe('var(--space-sm)');
  });

  it('leaves the desktop row as one line', () => {
    for (const bar of BARS) {
      for (const rule of desktopRules(bar)) {
        expect(rule.props.has('flex-wrap'), `${rule.selector} wraps at every width`).toBe(false);
      }
    }
    for (const text of BARS.map(b => `${b}-text`)) {
      for (const rule of desktopRules(text)) {
        // Read the shorthand too: `flex: 1 1 100%` stacks the bar just as well.
        const basis = rule.props.get('flex-basis') ?? rule.props.get('flex') ?? '';
        expect(basis, `${rule.selector} takes the whole line at every width`).not.toContain('100%');
      }
    }
  });
});
