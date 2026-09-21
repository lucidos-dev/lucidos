/**
 * The two webhook bars give their sentence the whole bar width on a phone.
 *
 * Photographed on a real iPhone in portrait: the refusal notice wrapped inside
 * roughly 60% of the bar. It ran to ten lines, beside a tall empty gap. The
 * button cannot shrink, so on a narrow viewport it keeps a column the sentence
 * pays for.
 *
 * Four properties, each a property of the RULE rather than of a rendered frame.
 * That is cheaper to assert here than in a browser.
 *
 * 1. ONE grouped rule covers both bars. The pair shares every other declaration
 *    on purpose, and a second copy is how the two drift apart.
 * 2. The sentence takes a whole line of its own, so the buttons wrap below it.
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
const MOBILE = '@media (max-width: 768px)';

/** The one rule that styles `className` below the mobile breakpoint. Asserting
 *  there is exactly one is half the point: a second copy would be the drift. */
function stackRule(className: string): CssRule {
  const found = rulesTargeting(shellCss, className).filter(r => r.atRules === MOBILE);
  expect(found, `${className} takes ${found.length} mobile rules, not one`).toHaveLength(1);
  return found[0];
}

/** Every rule styling `className` OUTSIDE the mobile breakpoint. */
function desktopRules(className: string): CssRule[] {
  return rulesTargeting(shellCss, className).filter(r => !r.atRules.includes('max-width: 768px'));
}

describe('the webhook bars stack on a phone', () => {
  it('wraps both bars from one rule, so the pair cannot drift apart', () => {
    const bar = stackRule('refusal-banner');
    expect(selectorList(bar.selector)).toEqual(['.ingress-banner', '.refusal-banner']);
    expect(stackRule('ingress-banner').selector).toBe(bar.selector);
    expect(bar.props.get('flex-wrap')).toBe('wrap');
    // The buttons keep the trailing edge they hold on the desktop row.
    expect(bar.props.get('justify-content')).toBe('flex-end');
  });

  it('gives the sentence a line of its own, on both bars', () => {
    const text = stackRule('refusal-banner-text');
    expect(selectorList(text.selector)).toEqual(['.ingress-banner-text', '.refusal-banner-text']);
    expect(stackRule('ingress-banner-text').selector).toBe(text.selector);
    // The base rule's `flex: 1` leaves the basis at 0, so the text shares the
    // line. A full basis is what pushes everything after it onto the next one.
    expect(text.props.get('flex-basis')).toBe('100%');
  });

  it('spends a spacing token on the gap it opens', () => {
    // Stacked, the bar grows a vertical gap it never had. A literal here is out
    // of the live style remote's reach, same as on the connection bar.
    expect(stackRule('refusal-banner').props.get('row-gap')).toBe('var(--space-sm)');
  });

  it('leaves the desktop row as one line', () => {
    for (const bar of ['ingress-banner', 'refusal-banner']) {
      for (const rule of desktopRules(bar)) {
        expect(rule.props.has('flex-wrap'), `${rule.selector} wraps at every width`).toBe(false);
      }
    }
    for (const text of ['ingress-banner-text', 'refusal-banner-text']) {
      for (const rule of desktopRules(text)) {
        // Read the shorthand too: `flex: 1 1 100%` stacks the bar just as well.
        const basis = rule.props.get('flex-basis') ?? rule.props.get('flex') ?? '';
        expect(basis, `${rule.selector} takes the whole line at every width`).not.toContain('100%');
      }
    }
  });
});
