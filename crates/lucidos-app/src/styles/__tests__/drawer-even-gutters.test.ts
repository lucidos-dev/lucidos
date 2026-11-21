/**
 * The menu drawer keeps the same gap on both sides of its rows on desktop.
 *
 * Two parts make the gaps equal. The panel hugs its widest row, so the row's
 * right padding is the gap after the widest label or badge. And the row's left
 * padding floor matches its right padding, so the gap before the icon is the
 * same. A width floor on the panel breaks the first part: a short menu then
 * leaves extra room after the labels.
 *
 * The phone layout overrides the left padding on purpose, so this pins only
 * the base rule.
 */
import { describe, it, expect } from 'vitest';
import postcss from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const mobileCss: string = readFileSync(resolve(here, '../mobile.css'), 'utf-8');

describe('menu drawer side gaps', () => {
  it('the panel hugs its widest row, with no width floor', () => {
    const panelRules = rulesTargeting(mobileCss, 'drawer');
    expect(panelRules.length).toBeGreaterThan(0);
    for (const rule of panelRules) {
      expect(rule.props.get('min-width'), `${rule.selector} sets a width floor`).toBeUndefined();
    }
    const base = panelRules.find(r => r.selector === '.drawer' && r.atRules === '');
    expect(base?.props.get('width')).toBe('fit-content');
  });

  it('the row pads its left side by the same floor as its right side', () => {
    const row = cssRules(mobileCss).find(
      r => r.selector === '.drawer-item' && r.atRules === '' && r.props.has('padding'),
    );
    const [, right, , left] = postcss.list.space(row?.props.get('padding') ?? '');
    expect(left).toBe(`max(${right}, var(--safe-area-left))`);
  });
});
