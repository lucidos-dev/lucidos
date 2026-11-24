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
    expect(rule.props.get('outline')).toBe('0.125rem solid transparent');
    expect(rule.props.get('outline-offset')).toBe('-0.125rem');
  });

  it('the settings nav row draws the shared band outward, into the panel gutter', () => {
    const rule = focusRule('settings-nav-row');
    expect(rule.props.get('box-shadow')).toBe('var(--focus-ring)');
    expect(rule.props.get('outline')).toBe('0.125rem solid transparent');
  });
});
