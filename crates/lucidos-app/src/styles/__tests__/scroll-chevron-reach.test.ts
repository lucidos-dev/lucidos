/**
 * On touch the floating scroll chevrons take a tap that lands near their
 * circle. The circle is drawn and placed, so it keeps its size; a transparent
 * `::before` reaches past it instead. Without it, a near miss lands on the step
 * row's context counter underneath and opens the context viewer.
 *
 * e2e/scroll-chevron-hit-target-mobile.spec.ts measures the painted result.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string => readFileSync(resolve(here, '..', rel), 'utf-8');

const baseCss = styles('global/base.css');
const mobileCss = styles('mobile.css');
const chevronCss = styles('chat/input-messages.css');

const TOUCH = '@media (pointer: coarse)';
const REACH = '.scroll-to-top::before, .scroll-to-bottom::before';

/** The one rule in `css` with this selector at this at-rule scope. */
function rule(css: string, selector: string, atRules = ''): Map<string, string> {
  const found = cssRules(css).filter(r => r.selector === selector && r.atRules === atRules);
  expect(found.length, `expected exactly one \`${selector}\` at "${atRules}"`).toBe(1);
  return found[0].props;
}

/** A plain `<n>rem` value, in rem. */
function rem(value: string | undefined): number {
  expect(value).toMatch(/^[\d.]+rem$/);
  return parseFloat(value!);
}

const touchRoot = (): Map<string, string> => rule(baseCss, ':root', TOUCH);
const reach = (): number => rem(touchRoot().get('--scroll-chevron-reach'));

describe('the scroll chevrons reach past their circle on touch', () => {
  it('make a target of at least 44px, plus a margin, at the 16px root', () => {
    // The circle is the mobile header's icon box on touch.
    expect(touchRoot().get('--scroll-chevron-size')).toBe('var(--mobile-header-icon-box)');
    const box = rem(rule(mobileCss, ':root').get('--mobile-header-icon-box'));
    // `inset` counts from the padding box, so the border comes off each side.
    const border = /^(\d+)px /.exec(rule(chevronCss, '.scroll-to-top, .scroll-to-bottom').get('border') ?? '')?.[1];
    expect(border, 'the circle\'s border is no longer a px width').toBeDefined();
    expect((box + 2 * reach()) * 16 - 2 * Number(border)).toBeGreaterThan(44);
  });

  it('is a transparent overlay derived from the reach, on touch only', () => {
    const before = rule(chevronCss, REACH, TOUCH);
    expect(before.get('content')).toBe("''");
    expect(before.get('position'), 'absolute, so it moves neither the circle nor the glyph').toBe('absolute');
    expect(before.get('inset')).toBe('calc(-1 * var(--scroll-chevron-reach))');
    expect(before.has('background'), 'the reach is never painted').toBe(false);
    expect(cssRules(chevronCss).filter(r => r.selector.includes('::before') && r.selector.includes('scroll-to-')))
      .toHaveLength(1);
  });

  it('takes no tap while its chevron is hidden', () => {
    // The overlay inherits `pointer-events` from its button. A value of its
    // own would keep a faded chevron catching taps meant for the counters.
    expect(rule(chevronCss, REACH, TOUCH).has('pointer-events')).toBe(false);
    expect(rule(chevronCss, '.scroll-to-top, .scroll-to-bottom').get('pointer-events')).toBe('none');
    expect(rule(chevronCss, '.scroll-to-top.visible, .scroll-to-bottom.visible').get('pointer-events')).toBe('auto');
  });

  it('paints, and hit-tests, above the transcript', () => {
    // The chevrons are siblings of `.thread-content`, which is z-index 2 on
    // mobile. The overlay rides the button's own layer, so it has no z-index.
    expect(rule(chevronCss, REACH, TOUCH).has('z-index')).toBe(false);
    expect(rule(chevronCss, '.scroll-to-top, .scroll-to-bottom').get('z-index')).toBe('calc(var(--z-float) + 1)');
    const zFloat = Number(rule(baseCss, ':root').get('--z-float'));
    const transcriptZ = Number(rule(mobileCss, '.mobile-swipe-pane .thread-content', '@media (max-width: 768px)').get('z-index'));
    expect(zFloat + 1).toBeGreaterThan(transcriptZ);
  });

  it('stops at the gap each chevron keeps to the chrome beside it', () => {
    // The up chevron sits a gap below the title bar and the down chevron a gap
    // above the composer. A longer reach would take their taps.
    const downGap = rem(rule(chevronCss, '.scroll-to-bottom').get('bottom'));
    const upTop = rule(mobileCss, '.mobile-swipe-pane .scroll-to-top', '@media (max-width: 768px)').get('top') ?? '';
    const upGap = rem(/\+ ([\d.]+rem)\)$/.exec(upTop)?.[1]);
    expect(reach()).toBeLessThanOrEqual(downGap);
    expect(reach()).toBeLessThanOrEqual(upGap);
  });
});
