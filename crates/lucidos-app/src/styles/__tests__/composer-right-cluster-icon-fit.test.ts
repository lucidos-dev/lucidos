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

const nudges: CssRule[] = iconRules.filter((r) => r.props.has('transform')
  && r.selector.includes('.prompt-actions-row')
  && !r.selector.includes('.prompt-actions-right'));
const rightGroup: CssRule[] = iconRules.filter((r) => r.selector.includes('.prompt-actions-right'));

describe('the composer row still nudges its leading icons', () => {
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

  it('hands back the block size the oversized tap target takes', () => {
    expect(rightGroup[0].props.get('margin-block')).toMatch(/^-/);
  });

  it('comes after the nudge, which it ties with on specificity', () => {
    expect(iconRules.indexOf(rightGroup[0])).toBeGreaterThan(iconRules.indexOf(nudges[0]));
  });
});
