import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const sheet = (name: string): string => readFileSync(resolve(here, '..', name), 'utf-8');
const rules = cssRules(sheet('section-header.css'));
const TRIMMED = '@supports (text-box-trim: trim-both)';
const rule = (selector: string, atRules = '') =>
  rules.find(r => r.selector === selector && r.atRules === atRules);

const COUNT = '.list-section-title .section-count';
const BADGE = '.list-section-title .section-count-badge';
const COLLAPSED_BADGE = '.list-section-title-collapsible.collapsed .section-count-badge';
const PILL = `${BADGE}::before`;
const COLLAPSED_PILL = `${COLLAPSED_BADGE}::before`;
const OPEN = '.section-count-open';
const COLLAPSED_OPEN = '.list-section-title-collapsible.collapsed .section-count-open';

describe('section count pill', () => {
  it('moves the fill off the badge onto the pill layer', () => {
    expect(rule(BADGE)?.props.get('background')).toBe('none');
    expect(rule(PILL)?.props.get('background')).toBe('var(--section-count-fill)');
  });

  it('hides the pill on an open section and shows it on a collapsed one', () => {
    expect(rule(PILL)?.props.get('opacity')).toBe('0');
    expect(rule(COLLAPSED_PILL)?.props.get('opacity')).toBe('1');
  });

  it('shows exactly one copy of the number in each state', () => {
    expect(rule(BADGE)?.props.get('visibility')).toBe('hidden');
    expect(rule(COLLAPSED_BADGE)?.props.get('visibility')).toBe('visible');
    expect(rule(COLLAPSED_OPEN)?.props.get('visibility')).toBe('hidden');
    expect(rule(OPEN)?.props.has('visibility')).toBe(false);
    for (const r of rulesTargeting(sheet('section-header.css'), 'section-count')) {
      expect(r.props.get('color'), r.selector).not.toBe('transparent');
    }
  });

  it('lets the pill fade out through the hidden badge', () => {
    expect(rule(PILL)?.props.get('visibility')).toBe('visible');
  });

  it('draws the open copy one type step up, over the pill\'s box', () => {
    expect(rule(COUNT)?.props.get('--section-count-open-scale')).toBe('1.2');
    expect(rule(OPEN)?.props.get('font-size')).toBe('var(--font-size-sm)');
    expect(rule(OPEN)?.props.get('position')).toBe('absolute');
    expect(rule(BADGE)?.props.has('font-size')).toBe(false);
  });

  it('rests both copies untransformed', () => {
    expect(rule(COLLAPSED_BADGE)?.props.get('scale')).toBe('none');
    expect(rule(OPEN)?.props.has('scale')).toBe(false);
    expect(rule(OPEN)?.props.has('translate')).toBe(false);
    expect(rule(BADGE)?.props.get('scale')).toBe('var(--section-count-open-scale)');
    expect(rule(COLLAPSED_OPEN)?.props.get('scale'))
      .toBe('calc(1 / var(--section-count-open-scale))');
  });

  it('grows each copy from its own whole-pixel baseline', () => {
    expect(rule(BADGE, TRIMMED)?.props.get('transform-origin'))
      .toBe('50% calc(100% - var(--badge-baseline-inset))');
    const open = rule(OPEN, TRIMMED);
    expect(open?.props.get('text-box-trim')).toBe('trim-end');
    expect(open?.props.get('text-box-edge')).toBe('cap alphabetic');
    expect(open?.props.get('bottom')).toBe('round(50% - 0.5 * 1cap, 1px)');
    expect(open?.props.get('transform-origin')).toBe('50% 100%');
  });

  it('moves the baseline in one step, never mid-animation', () => {
    const numberRules = ['section-count-badge', 'section-count-open']
      .flatMap(cls => rulesTargeting(sheet('section-header.css'), cls))
      .concat(rulesTargeting(sheet('badges.css'), 'section-count-badge'))
      .filter(r => !r.selector.includes('::'));
    expect(numberRules.length).toBeGreaterThan(0);
    for (const r of numberRules) {
      const transition = r.props.get('transition') ?? '';
      expect(transition, r.selector).not.toMatch(/\b(translate|transform|bottom|all)\b/);
      expect(r.props.has('transform'), r.selector).toBe(false);
    }
  });

  it('animates each copy on its way in, on scaled duration tokens', () => {
    for (const r of [rule(BADGE), rule(COLLAPSED_BADGE), rule(OPEN)]) {
      expect(r?.props.get('transition') ?? '').toMatch(/^scale var\(--duration-/);
    }
    expect(rule(COLLAPSED_OPEN)?.props.get('transition')).toBe('none');
    for (const r of [rule(PILL), rule(COLLAPSED_PILL)]) {
      const transition = r?.props.get('transition') ?? '';
      expect(transition).toMatch(/transform var\(--duration-/);
      expect(transition).toMatch(/opacity var\(--duration-/);
    }
  });
});
