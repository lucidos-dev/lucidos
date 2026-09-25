/**
 * A drawer section header draws its count as a pill only while the section is
 * collapsed. An open section shows a bare number, one type step larger. The
 * pill is a `::before` layer, so it can animate apart from the digits.
 */
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
const rules = cssRules(sheet('drawer.css'));
const rule = (selector: string) => rules.find(r => r.selector === selector && r.atRules === '');

const BADGE = '.list-section-title-collapsible > .section-count-badge';
const COLLAPSED_BADGE = '.list-section-title-collapsible.collapsed > .section-count-badge';
const PILL = `${BADGE}::before`;
const COLLAPSED_PILL = '.list-section-title-collapsible.collapsed > .section-count-badge::before';

describe('section count pill', () => {
  it('moves the fill off the badge onto the pill layer', () => {
    expect(rule(BADGE)?.props.get('background')).toBe('none');
    expect(rule(PILL)?.props.get('background')).toBe('var(--section-count-fill)');
  });

  it('hides the pill on an open section and shows it on a collapsed one', () => {
    expect(rule(PILL)?.props.get('opacity')).toBe('0');
    expect(rule(COLLAPSED_PILL)?.props.get('opacity')).toBe('1');
  });

  it('draws the bare number larger by transform, so the row keeps its height', () => {
    expect(rule(BADGE)?.props.get('transform')).toBe('scale(1.2)');
    expect(rule(BADGE)?.props.has('font-size')).toBe(false);
    expect(rule(COLLAPSED_BADGE)?.props.get('transform')).toBe('none');
  });

  it('keeps the bare number on the pill\'s centre', () => {
    // The default origin is the box's centre, where the pill's digits sit.
    const badgeRules = ['drawer.css', 'badges.css']
      .flatMap(name => rulesTargeting(sheet(name), 'section-count-badge'));
    expect(badgeRules.length).toBeGreaterThan(0);
    for (const r of badgeRules) {
      expect(r.props.has('transform-origin'), r.selector).toBe(false);
    }
  });

  it('animates both ways on scaled duration tokens', () => {
    for (const r of [rule(BADGE), rule(COLLAPSED_BADGE)]) {
      expect(r?.props.get('transition') ?? '').toMatch(/transform var\(--duration-/);
    }
    for (const r of [rule(PILL), rule(COLLAPSED_PILL)]) {
      const transition = r?.props.get('transition') ?? '';
      expect(transition).toMatch(/transform var\(--duration-/);
      expect(transition).toMatch(/opacity var\(--duration-/);
    }
  });
});
