/**
 * The threads Filter's fades, which jsdom cannot run, pinned by a CSS scan.
 *
 * Two timings, on purpose:
 * - THE BUTTON. The glyph, its badge and the pressed highlight are button
 *   feedback, on `--duration-fast` like every header icon.
 * - THE VIEW. Threads and Filters dip through the pane background on
 *   `--duration-slow`: the shared navigation cover rises, holds while the views
 *   swap, and clears. The pane title switches word at once.
 *
 * One guard is a specific failure: THE RIM. The header paints a resting icon in
 * translucent white. Two translucent shapes stacked mid-crossfade paint their
 * overlap twice, so the funnel's rim flares. The translucency lives on the
 * glyph WRAPPER instead, and the shapes paint opaque.
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

describe('the button fades on --duration-fast', () => {
  it.each([
    ['the glyph crossfade', host, '.crossfade-layer', 'opacity var(--duration-fast) ease'],
    ['the glyph wrapper (hover and pressed)', shell, '.app-header .filter-glyph', 'opacity var(--duration-fast) ease'],
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

/** The one @keyframes block with this name, as its raw body. */
function keyframes(name: string): string {
  const css = readFileSync(resolve(here, '..', 'global/host-components.css'), 'utf-8');
  const m = css.match(new RegExp(`@keyframes ${name}\\s*\\{([\\s\\S]*?)\\n\\}`));
  expect(m, `no @keyframes ${name}`).not.toBeNull();
  return m![1];
}

describe('the view dips on one timing', () => {
  it('the drawer cover dips on --duration-slow, resting transparent', () => {
    const dip = rule(host, '.nav-cover.nav-cover-dip');
    expect(dip.props.get('animation')).toBe('nav-cover-dip var(--duration-slow) ease-in-out forwards');
    // A late first frame shows the leaving view, never a flash of background.
    expect(dip.props.get('opacity')).toBe('0');
  });

  it('the cover holds opaque across the midpoint, where the views swap', () => {
    const body = keyframes('nav-cover-dip').replace(/\s+/g, ' ');
    expect(body).toContain('0% { opacity: 0; }');
    expect(body).toContain('45%, 55% { opacity: 1; }');
    expect(body).toContain('100% { opacity: 0; }');
  });

  it('the views swap at the midpoint, on the same token', () => {
    const swap = rule(drawer, '.thread-drawer > .thread-drawer-list, .thread-filter-cover');
    expect([...swap.props.entries()]).toEqual([
      ['transition', 'visibility 0s linear calc(var(--duration-slow) / 2)'],
    ]);
  });

  it("the drawer cover's fuse mirrors --duration-slow, so it outlives the dip at any speed", () => {
    const cover = readFileSync(resolve(here, '..', '../components/shared/NavigationCover.tsx'), 'utf-8');
    expect(cover).toMatch(/dip: \{ class: 'nav-cover nav-cover-dip', animMs: 300 \}/);
    const base = readFileSync(resolve(here, '..', 'global/base.css'), 'utf-8');
    expect(base).toMatch(/--duration-slow: calc\(0\.3s \* var\(--duration-scale\)\);/);
  });

  it('the content pane keeps its arrival cover', () => {
    expect(rule(host, '.nav-cover').props.get('animation'))
      .toBe('nav-cover-clear var(--duration-normal) ease-out forwards');
  });

  it('the drawer hosts the shared cover rather than a copy', () => {
    const hostRule = rule(drawer, '.thread-drawer > .nav-cover');
    expect([...hostRule.props.keys()]).toEqual(['z-index']);
    for (const r of drawer.filter(r => r.selector.includes('cover'))) {
      expect(r.props.has('animation'), r.selector).toBe(false);
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
    // The header tokens live in the theme blocks, where a theme can reach them.
    const bar = sheet('global/base.css').find(r => r.props.has('--header-fg-muted-alpha'));
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

// A fade lifts its element onto a compositing layer for as long as it runs,
// and a layer snaps to whole pixels. The button sits at a fractional y. So on a
// real iPhone the glyph and badge can hop as a fade begins and drop back after.
// Emulators paint without that compositing, so this scan is the guard.
describe('nothing in the button hops when a fade starts or ends', () => {
  it.each([
    '.app-header .filter-glyph',
    '.app-header .filter-glyph > .crossfade-layer',
    '.app-header .badge.filter-badge',
  ])('%s stays on its own layer at rest', (selector) => {
    // The badge also names `transform`, for badges.css (badge-glyph-centring.test.ts).
    const hints = (rule(shell, selector).props.get('will-change') ?? '').split(/\s*,\s*/);
    expect(hints).toContain('opacity');
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

describe('the views swap under the cover', () => {
  it('the shut cover is hidden and takes no pointer', () => {
    const shut = rule(drawer, '.thread-filter-cover');
    expect(shut.props.get('visibility')).toBe('hidden');
    expect(shut.props.get('pointer-events')).toBe('none');
    expect(shut.props.get('background')).toBe('var(--bg-primary)');
  });

  it('the open cover shows', () => {
    const open = rule(drawer, '.thread-filter-cover[data-open]');
    expect(open.props.get('visibility')).toBe('visible');
    expect(open.props.get('pointer-events')).toBe('auto');
  });

  it('the list hides while the panel is up, since both wear the same geometry', () => {
    const hides = rule(drawer, '.thread-drawer:has(> .thread-filter-cover[data-open]) > .thread-drawer-list');
    expect(hides.props.get('visibility')).toBe('hidden');
  });

  it('nothing in the swap carries a fade of its own: the cover is the one fade', () => {
    // The rules styling the two views' own boxes, not the rows inside them.
    // WebKit re-compositing content up from transparent is the iOS paint-loss
    // shape, so only `visibility` may transition here.
    const swapping = /(\.thread-filter-cover|\.thread-drawer-list)(\[data-open\])?$/;
    const boxes = drawer.filter(r => selectorList(r.selector).some(s => swapping.test(s)));
    // Guard the guard: the cover, the open cover, the hidden list and the swap.
    expect(boxes.length).toBeGreaterThanOrEqual(4);
    for (const r of boxes) {
      expect(r.props.has('opacity'), r.selector).toBe(false);
      const transition = r.props.get('transition');
      if (transition && transition !== 'none') expect(transition, r.selector).toMatch(/^visibility 0s /);
    }
  });
});

describe('reduced motion', () => {
  const reduce = (rules: CssRule[], selector: string) => {
    const found = rules.find(r => selectorList(r.selector).includes(`${REDUCED_MOTION_ROOT} ${selector}`));
    expect(found, `no reduced-motion rule for ${selector}`).toBeDefined();
    return found!;
  };

  // A scaled duration is short, not instant: the transition still waits for
  // its start, and Chromium held a fade at its old value for a frame under
  // load. So every button fade is cancelled outright.
  it.each([
    [host, '.crossfade-layer'],
    [shell, '.app-header .filter-glyph'],
    [shell, '.app-header .badge.filter-badge'],
  ])('cancels the button fade on %#', (rules, selector) => {
    expect(reduce(rules, selector).props.get('transition')).toBe('none');
  });

  it('swaps the views at once, with no delay', () => {
    expect(reduce(drawer, '.thread-drawer > .thread-drawer-list').props.get('transition')).toBe('none');
    expect(reduce(drawer, '.thread-filter-cover').props.get('transition')).toBe('none');
  });

  it('drops the cover to transparent', () => {
    const cover = reduce(host, '.nav-cover');
    expect(cover.props.get('animation')).toBe('none');
    expect(cover.props.get('opacity')).toBe('0');
  });
});

const hoversOf = (row: string) =>
  drawer.filter(r => selectorList(r.selector).some(s => s.startsWith(`${row}:hover`)));

describe('panel rows paint no band on touch', () => {
  // A touch screen keeps :hover on the last row tapped, so an unguarded hover
  // band stayed on that row after the tap. The tap highlight is off app-wide
  // (touch-paints-no-press.test.ts).
  it('.drawer-view-option hovers on a pointer only', () => {
    const hovers = hoversOf('.drawer-view-option');
    expect(hovers.length, 'no hover rule for .drawer-view-option').toBeGreaterThan(0);
    for (const h of hovers) expect(h.atRules, h.selector).toContain('@media (hover: hover)');
  });
});

// The band says "this row selects and switches the list at once", which is
// true of a status row and false of a checkbox.
it('checkbox rows paint no hover band', () => {
  expect(hoversOf('.thread-filter-option')).toEqual([]);
});
