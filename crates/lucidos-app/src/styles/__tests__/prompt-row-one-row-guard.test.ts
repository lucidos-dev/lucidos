import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const composerCss: string = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf-8');

const rowRules: CssRule[] = cssRules(composerCss).filter(
  (r) => r.selector.includes('.prompt-actions-row') || r.selector.includes('.prompt-actions-right'),
);

describe('no member of the row may shrink', () => {
  const noShrink: string[] = rowRules
    .filter((r) => r.props.get('flex-shrink') === '0')
    .flatMap((r) => r.selector.split(',').map((one) => one.trim()));

  it('declares flex-shrink: 0 for the row\'s own children', () => {
    expect(noShrink, 'nothing stops the row\'s members shrinking')
      .toContain('.prompt-actions-row > *');
  });

  it('reaches the cluster\'s children too', () => {
    expect(noShrink, 'nothing stops the cluster\'s buttons shrinking')
      .toContain('.prompt-actions-right > *');
  });
});

describe('the row can never draw a second line', () => {
  it('declares no wrap anywhere in the row', () => {
    const wrapped = rowRules.filter((r) => {
      const wrap = r.props.get('flex-wrap') ?? r.props.get('flex-flow');
      return wrap !== undefined && !wrap.includes('nowrap');
    });
    expect(wrapped.map((r) => r.selector), 'a wrap lets a member onto a second line')
      .toEqual([]);
  });

  it('declares no column direction anywhere in the row', () => {
    const columns = rowRules.filter((r) => (r.props.get('flex-direction') ?? '').startsWith('column'));
    expect(columns.map((r) => r.selector)).toEqual([]);
  });

  it('keeps the retired sub-row classes retired', () => {
    const rules = composerCss.replace(/\/\*[\s\S]*?\*\//g, '');
    expect(rules).not.toMatch(/\.prompt-actions-subrow\b/);
    expect(rules).not.toMatch(/\.is-stacked\b/);
  });
});

describe('the row holds its height whatever stands', () => {
  it('is one icon tap target tall, plus its bottom padding', () => {
    const row = rowRules.find((r) => r.selector === '.prompt-actions-row' && r.props.has('min-height'));
    expect(row?.props.get('min-height')).toBe('calc(var(--icon-tap-target) + 0.5rem)');
    expect(row?.props.get('padding')).toMatch(/ 0\.5rem [^ ]+$/);
  });
});
