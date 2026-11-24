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

const MOBILE = '@media (--phone-layout)';

const BARS = ['ingress-banner', 'refusal-banner', 'sandbox-banner'];

function stackRule(className: string): CssRule {
  const found = rulesTargeting(shellCss, className).filter(r => r.atRules === MOBILE);
  expect(found, `${className} takes ${found.length} mobile rules, not one`).toHaveLength(1);
  return found[0];
}

function desktopRules(className: string): CssRule[] {
  return rulesTargeting(shellCss, className).filter(r => !r.atRules.includes('(--phone-layout)'));
}

describe('the webhook and sandbox bars stack on a phone', () => {
  it('wraps every bar from one rule, so they cannot drift apart', () => {
    const bar = stackRule('refusal-banner');
    expect(selectorList(bar.selector)).toEqual(BARS.map(b => `.${b}`));
    for (const other of BARS) expect(stackRule(other).selector).toBe(bar.selector);
    expect(bar.props.get('flex-wrap')).toBe('wrap');
    expect(bar.props.get('justify-content')).toBe('flex-end');
  });

  it('reserves room for the leading icon on every bar', () => {
    const text = stackRule('refusal-banner-text');
    expect(selectorList(text.selector)).toEqual(BARS.map(b => `.${b}-text`));
    for (const other of BARS) expect(stackRule(`${other}-text`).selector).toBe(text.selector);
    expect(text.props.get('flex-basis')).toBe('calc(100% - var(--icon-size-md) - var(--space-md))');
  });

  it('spends a spacing token on the gap it opens', () => {
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
        const basis = rule.props.get('flex-basis') ?? rule.props.get('flex') ?? '';
        expect(basis, `${rule.selector} takes the whole line at every width`).not.toContain('100%');
      }
    }
  });
});
