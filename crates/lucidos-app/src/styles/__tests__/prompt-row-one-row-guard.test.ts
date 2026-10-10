/** The composer row is ONE row, and no member may shrink.
 *
 *  Both are structural rather than reconciled, which is the whole point. A
 *  member that can be squeezed reports a width smaller than it needs. The fold
 *  then concludes the row fits, while a nowrap label runs off the screen. And a
 *  rule that can draw a second line is a second line waiting to happen.
 *
 *  A source scan because both are cascade-resolved, which jsdom does not do.
 *  The rendered half is `e2e/composer-row-fit-mobile.spec.ts`, which sweeps
 *  widths and scales against the real box.
 *
 *  Plan: `docs/plans/2026-09-19-the-composer-row-is-one-row.md`. */
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

/** Every rule whose selector NAMES the row or its right-hand cluster, subject
 *  or not. `rulesTargeting` is the wrong reader here: it keeps only rules whose
 *  subject is the class, and the no-shrink rule's subject is the child. */
const rowRules: CssRule[] = cssRules(composerCss).filter(
  (r) => r.selector.includes('.prompt-actions-row') || r.selector.includes('.prompt-actions-right'),
);

describe('no member of the row may shrink', () => {
  /** `.action-btn` sets `min-width: 3.5rem`, which REPLACES a flex item's
   *  automatic min-content floor. Without this rule an Archive button squeezes
   *  to 3.5rem under pressure and its label spills out of the box. The fold
   *  reads the squeezed width and folds nothing. */
  /** One selector per list entry, because a rule states them comma-joined. */
  const noShrink: string[] = rowRules
    .filter((r) => r.props.get('flex-shrink') === '0')
    .flatMap((r) => r.selector.split(',').map((one) => one.trim()));

  it('declares flex-shrink: 0 for the row\'s own children', () => {
    expect(noShrink, 'nothing stops the row\'s members shrinking')
      .toContain('.prompt-actions-row > *');
  });

  /** The cluster is a member AND a flex container, so its own children would
   *  shrink inside it whatever the row says. */
  it('reaches the cluster\'s children too', () => {
    expect(noShrink, 'nothing stops the cluster\'s buttons shrinking')
      .toContain('.prompt-actions-right > *');
  });
});

describe('the row can never draw a second line', () => {
  /** Three mechanisms used to disagree about how the row gives way: the fold,
   *  an `is-stacked` column lift, and this wrap. The fold is the only one now,
   *  and the other two are gone rather than kept as a floor. */
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

  /** The lift's own classes. A rule for either is a second row reintroduced,
   *  whatever it declares. */
  it('keeps the retired sub-row classes retired', () => {
    const rules = composerCss.replace(/\/\*[\s\S]*?\*\//g, '');
    expect(rules).not.toMatch(/\.prompt-actions-subrow\b/);
    expect(rules).not.toMatch(/\.is-stacked\b/);
  });
});
