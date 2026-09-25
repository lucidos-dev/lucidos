/**
 * The Filter button, its badge, the pane title and the filter panel change
 * together, on one timing: an opacity fade over `--duration-fast`. This scan
 * pins the rules that make that true, since jsdom runs no CSS.
 *
 * Two of them guard a specific failure:
 * - THE RIM. The header paints a resting icon in translucent white. Two
 *   translucent shapes stacked mid-crossfade paint their overlap twice, so the
 *   funnel's rim flares. The translucency lives on the glyph WRAPPER instead,
 *   and the shapes paint opaque.
 * - THE FADE THROUGH. The panel wears the list's geometry, so a crossfade
 *   prints rows and options on the same lines. The opaque cover shows at once
 *   and hides only after the fade layer inside it is done.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { REDUCED_MOTION_ROOT, cssRules, selectorList, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const sheet = (rel: string): CssRule[] => cssRules(readFileSync(resolve(here, '..', rel), 'utf-8'));

const shell = sheet('panels/shell.css');
const host = sheet('global/host-components.css');
const drawer = sheet('drawer.css');

/** The one rule with exactly this selector. */
function rule(rules: CssRule[], selector: string): CssRule {
  const found = rules.filter(r => r.selector === selector);
  expect(found, `expected one rule for ${selector}`).toHaveLength(1);
  return found[0];
}

describe('one timing for every Filter fade', () => {
  it.each([
    ['the glyph and title crossfade', host, '.crossfade-layer', 'opacity var(--duration-fast) ease'],
    ['the glyph wrapper (hover and pressed)', shell, '.app-header .filter-glyph', 'opacity var(--duration-fast) ease'],
    ['the panel options', drawer, '.thread-filter-fade', 'opacity var(--duration-fast) ease'],
  ])('%s fade on --duration-fast', (_what, rules, selector, transition) => {
    expect(rule(rules, selector).props.get('transition')).toBe(transition);
  });

  it('the pressed highlight lands on the same timing as the rest', () => {
    expect(rule(shell, '.app-header .icon-btn.filter-btn').props.get('transition-duration'))
      .toBe('var(--duration-fast)');
  });

  it('the badge fades in and out on it, ring included', () => {
    for (const selector of ['.app-header .badge.filter-badge', '.app-header .badge.filter-badge[data-shown]']) {
      const transition = rule(shell, selector).props.get('transition') ?? '';
      expect(transition).toContain('opacity var(--duration-fast) ease');
      expect(transition).toContain('box-shadow var(--duration-fast)');
    }
  });
});

describe('the rim is painted once', () => {
  it('the wrapper carries the alpha, and the shapes inside paint opaque', () => {
    const glyph = rule(shell, '.app-header .filter-glyph');
    expect(glyph.props.get('opacity')).toBe('var(--header-fg-muted-alpha)');
    expect(glyph.props.get('color')).toBe('var(--header-fg)');
  });

  it('the muted tone and the wrapper read one alpha', () => {
    const bar = shell.find(r => r.props.has('--header-fg-muted-alpha'));
    expect(bar, 'no rule declares --header-fg-muted-alpha').toBeDefined();
    expect(bar!.props.get('--header-fg-muted')).toContain('var(--header-fg-muted-alpha)');
  });

  it('no rule paints a shape inside the stack translucent', () => {
    for (const r of [...shell, ...host]) {
      if (!r.selector.includes('filter-glyph')) continue;
      const color = r.props.get('color');
      if (color) expect(color, r.selector).toBe('var(--header-fg)');
    }
  });

  it('hover and pressed raise the wrapper to opaque, as they do the other icons', () => {
    expect(rule(shell, '.app-header .icon-btn.view-selector-active .filter-glyph').props.get('opacity')).toBe('1');
    expect(rule(shell, '.app-header .icon-btn.filter-btn:hover:where(:not(:disabled)) .filter-glyph').props.get('opacity'))
      .toBe('1');
  });
});

describe('the badge takes no pointer while hidden', () => {
  it('turns hidden at the end of a fade out and visible at the start of a fade in', () => {
    const hidden = rule(shell, '.app-header .badge.filter-badge');
    expect(hidden.props.get('visibility')).toBe('hidden');
    expect(hidden.props.get('transition')).toContain('visibility 0s linear var(--duration-fast)');
    const shown = rule(shell, '.app-header .badge.filter-badge[data-shown]');
    expect(shown.props.get('visibility')).toBe('visible');
    expect(shown.props.get('transition')).toMatch(/visibility 0s$/);
  });
});

describe('the panel fades through', () => {
  it('the shut cover hides only once the fade out is done', () => {
    const shut = rule(drawer, '.thread-filter-cover');
    expect(shut.props.get('visibility')).toBe('hidden');
    expect(shut.props.get('pointer-events')).toBe('none');
    expect(shut.props.get('transition')).toBe('visibility 0s linear var(--duration-fast)');
  });

  it('the open cover lands at once, opaque', () => {
    const open = rule(drawer, '.thread-filter-cover[data-open]');
    expect(open.props.get('visibility')).toBe('visible');
    expect(open.props.get('transition')).toBe('visibility 0s');
    expect(rule(drawer, '.thread-filter-cover').props.get('background')).toBe('var(--bg-primary)');
  });

  it('the options fade on a layer inside the scroller, so the scrollbar never fades', () => {
    expect(rule(drawer, '.thread-filter-cover').props.get('overflow-y')).toBe('scroll');
    expect(rule(drawer, '.thread-filter-fade').props.get('opacity')).toBe('0');
    expect(rule(drawer, '.thread-filter-cover[data-open] .thread-filter-fade').props.get('opacity')).toBe('1');
    expect(rule(drawer, '.thread-filter-cover').props.has('opacity')).toBe(false);
  });
});

describe('reduced motion', () => {
  // A scaled duration is short, not instant: the transition still waits for
  // its start, and Chromium held a fade at its old value for a frame under
  // load. So every Filter fade is cancelled outright.
  it.each([
    [host, '.crossfade-layer'],
    [shell, '.app-header .filter-glyph'],
    [shell, '.app-header .badge.filter-badge'],
    [drawer, '.thread-filter-cover'],
    [drawer, '.thread-filter-fade'],
  ])('cancels the fade on %#', (rules, selector) => {
    const cancel = rules.find(r => selectorList(r.selector).includes(`${REDUCED_MOTION_ROOT} ${selector}`));
    expect(cancel, `no reduced-motion rule for ${selector}`).toBeDefined();
    expect(cancel!.props.get('transition')).toBe('none');
  });
});

describe('panel rows paint no band on touch', () => {
  // A touch screen keeps :hover on the last row tapped, so an unguarded hover
  // band stayed on that row after the tap. iOS also flashes its own grey tap
  // highlight on a label or button unless it is turned off.
  it.each(['.thread-filter-option', '.drawer-view-option'])('%s hovers on a pointer only', (row) => {
    const hovers = drawer.filter(r => selectorList(r.selector).some(s => s.startsWith(`${row}:hover`)));
    expect(hovers.length, `no hover rule for ${row}`).toBeGreaterThan(0);
    for (const h of hovers) expect(h.atRules, h.selector).toContain('@media (hover: hover)');
    expect(rule(drawer, row).props.get('-webkit-tap-highlight-color')).toBe('transparent');
  });
});
