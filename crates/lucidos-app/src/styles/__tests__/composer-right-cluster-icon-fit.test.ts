/**
 * An icon in the composer's RIGHT-hand group must not set that group's height,
 * and must not wear the leading cluster's downward nudge.
 *
 * `.prompt-actions-row` sits its children on `align-items: flex-end` beside
 * shorter buttons. So a transform pushes every `.icon-btn.header-icon` in it
 * down, to bring the two sets of centres together. `.prompt-actions-right` is
 * laid out the other way round. It centres its own children, and it is only as
 * tall as a 1.5rem `.action-btn` beside a 1.625rem send button.
 *
 * So the nudge lands the glyph below its neighbours there, and the 2.25rem tap
 * target becomes the group's height. That second one is the sharp edge: the
 * send button then rises 5px the moment a *standing apply* appears and drops
 * again when it goes, under a thumb already aiming at it.
 *
 * A source scan because both halves are cascade-resolved, which jsdom does not
 * do. Nothing else in the gate would see it either: `tsc` skips CSS and
 * `vite build` fails only on a syntax error.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const composerCss: string = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf-8');

const iconRules: CssRule[] = rulesTargeting(composerCss, 'header-icon')
  .filter((r) => r.atRules === '');

/** The rules that nudge, and the rules that answer for the right-hand group. */
const nudges: CssRule[] = iconRules.filter((r) => r.props.has('transform')
  && r.selector.includes('.prompt-actions-row')
  && !r.selector.includes('.prompt-actions-right'));
const rightGroup: CssRule[] = iconRules.filter((r) => r.selector.includes('.prompt-actions-right'));

describe('the composer row still nudges its leading icons', () => {
  // Everything below is about undoing this rule. Without it the scan would
  // pass by describing a nudge nobody applies.
  it('pushes them down with a transform', () => {
    expect(nudges).toHaveLength(1);
    expect(nudges[0].props.get('transform')).toMatch(/^translateY\(/);
  });
});

describe('the right-hand group undoes both halves of it', () => {
  it('answers for its own icons', () => {
    expect(rightGroup).toHaveLength(1);
  });

  it('drops the nudge, which would push the glyph below its neighbours', () => {
    expect(rightGroup[0].props.get('transform')).toBe('none');
  });

  // The tap target overhangs the group instead of setting its height, so the
  // send button beside it stays where it was.
  it('hands back the block size the oversized tap target takes', () => {
    expect(rightGroup[0].props.get('margin-block')).toMatch(/^-/);
  });

  it('comes after the nudge, which it ties with on specificity', () => {
    expect(iconRules.indexOf(rightGroup[0])).toBeGreaterThan(iconRules.indexOf(nudges[0]));
  });
});
