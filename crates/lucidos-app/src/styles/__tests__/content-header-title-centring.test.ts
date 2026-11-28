import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, focusRingWidthRem, rulesTargeting, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const shellCss: string = readFileSync(resolve(stylesDir, 'panels/shell.css'), 'utf-8');
const shellRules = cssRules(shellCss);

const DESKTOP = '@media (--desktop-layout)';

function desktopRule(selector: string): CssRule {
  const found = shellRules.filter(r => r.selector === selector && r.atRules === DESKTOP);
  expect(found.length, `expected exactly one desktop \`${selector}\` rule`).toBe(1);
  return found[0];
}

const desktopRoot = (): CssRule => {
  const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
  expect(root, 'no desktop :root').toBeDefined();
  return root!;
};

const TITLE = '.app-header .pane-header-content-title';
const ROW = '.app-header .content-header-elements';

describe('the content title is centred on the row, not between the icons', () => {
  it('is out of the flow and pinned to the row\'s middle', () => {
    const title = desktopRule(TITLE);
    expect(title.props.get('position')).toBe('absolute');
    expect(title.props.get('left')).toBe('50%');
    expect(title.props.get('top')).toBe('50%');
    expect(title.props.get('transform')).toBe('translate(-50%, -50%)');
    expect(title.props.get('flex'), 'flex is what centred it on the gap').toBeUndefined();
    expect(title.props.get('min-width'), 'the clamp is the guarantee now').toBeUndefined();
  });

  it('leaves the hamburger and the actions pinned to the row\'s two ends', () => {
    expect(desktopRule(ROW).props.get('justify-content')).toBe('space-between');
    const ends = desktopRule(
      '.app-header .content-header-elements > .hamburger-panel, .app-header .content-header-actions',
    );
    expect(ends.props.get('flex-shrink'), 'only the title may give way').toBe('0');
  });

  it('clears the WIDER of the row\'s two ends on BOTH sides', () => {
    expect(desktopRule(TITLE).props.get('width')).toBe(
      'clamp( var(--desktop-nav-min-span), calc(100% - 2 * max(var(--content-side-reserve), calc(var(--content-leading-inset) + 2 * var(--header-icon-box) + 3 * 0.25rem))),'
      + ' clamp( var(--desktop-nav-span), calc(100% - 2 * var(--content-unfolded-reserve)),'
      + ' var(--desktop-content-title-span-max) ) )',
    );
  });

  it('holds the chevrons the same distance apart as the thread pane\'s until it grows', () => {
    const brand = desktopRule('.app-header .pane-header-brand .pane-header-brand-label');
    for (const token of ['--desktop-nav-min-span', '--desktop-nav-span']) {
      expect(brand.props.get('width'), `${token} is not shared`).toContain(token);
    }
    expect(brand.props.get('width'), 'the Conversation brand grows with the Canvas title')
      .not.toContain('--desktop-content-title-span-max');
  });

  it('grows on a wide pane, but only up to its cap', () => {
    const cap = desktopRoot().props.get('--desktop-content-title-span-max');
    const span = desktopRoot().props.get('--desktop-nav-span');
    expect(cap, '--desktop-content-title-span-max is gone').toBeDefined();
    expect(parseFloat(cap!), 'the cap does not let the title grow').toBeGreaterThan(parseFloat(span!));
  });

  it('states no height of its own, so it cannot hang out of the row', () => {
    const sized = rulesTargeting(shellCss, 'pane-header-content-title')
      .filter(r => r.props.has('height') || r.props.has('min-height'));
    expect(sized, 'the title box states a height of its own').toEqual([]);
  });

  it('keeps the trailing cluster painting above it', () => {
    const actions = desktopRule('.app-header .content-header-actions');
    expect(actions.props.get('position')).toBe('relative');
    expect(actions.props.get('z-index')).toBe('1');
  });

  it('a chevron on the clip edge draws its focus ring INSET', () => {
    const ring = desktopRule(
      `${TITLE} > .icon-btn:focus-visible, .app-header .pane-header-brand-label > .icon-btn:focus-visible`,
    );
    expect(ring.props.get('box-shadow')).toBe('inset var(--focus-ring)');
    expect(ring.props.get('outline-offset'), 'the forced-colors fallback still rings outward')
      .toBe('-0.125rem');
  });

  it('the ring band fits inside the button, clear of the glyph', () => {
    const iconBox = parseFloat(desktopRoot().props.get('--header-icon-box')!);
    const base = readFileSync(resolve(stylesDir, 'global/base.css'), 'utf-8');
    const glyph = parseFloat(/--icon-size-lg:\s*([\d.]+)rem/.exec(base)![1]);
    const band = focusRingWidthRem(base);
    expect((iconBox - glyph) / 2, 'the inset band would paint over the glyph')
      .toBeGreaterThan(band);
  });

  it('the chevrons are DIRECT children, so the rule holding their size applies', () => {
    expect(desktopRule(`${TITLE} > .icon-btn`).props.get('flex')).toBe('0 0 auto');
    expect(rulesTargeting(shellCss, 'header-title-span'), 'the retired wrapper still has rules')
      .toEqual([]);
    const appHeader = readFileSync(
      resolve(stylesDir, '../components/layout/AppHeader.tsx'), 'utf-8',
    );
    expect(appHeader, 'the retired wrapper is still rendered').not.toContain('header-title-span');
  });
});

describe('the reserve is derived from the controls, never guessed', () => {
  const reserve = (): string => {
    const value = desktopRoot().props.get('--content-side-reserve');
    expect(value, '--content-side-reserve is gone').toBeDefined();
    return value!;
  };

  it('reserves the persistent pairs, folding optional actions first', () => {
    expect(reserve()).toBe('calc(2 * var(--header-icon-box) + 3 * 0.25rem + var(--header-padding-x))');
  });

  it('counts the row\'s OWN gap, not a number that once matched it', () => {
    const gap = desktopRule(ROW).props.get('gap');
    expect(gap, 'the row declares no gap').toBeDefined();
    const term = /\+ 3 \* ([\d.]+rem) \+ var\(--header-padding-x\)\)$/.exec(reserve())?.[1];
    expect(term, `--content-side-reserve drifted from the row's ${gap} gap`).toBe(gap);
  });

  it('clears the minimal overflow and bell cluster', () => {
    const iconBox = parseFloat(desktopRoot().props.get('--header-icon-box')!);
    const gapRem = parseFloat(desktopRule(ROW).props.get('gap')!);
    const widestCluster = 2 * iconBox + gapRem;
    const boxes = Number(/calc\((\d+) \* var\(--header-icon-box\)/.exec(reserve())?.[1]);
    const gaps = Number(/\+ (\d+) \* [\d.]+rem \+ var\(--header-padding-x\)\)$/.exec(reserve())?.[1]);
    expect(boxes * iconBox + gaps * gapRem).toBeGreaterThanOrEqual(widestCluster + 2 * gapRem);
  });
});

describe('the title grows only out of room no action cluster needs', () => {
  const unfolded = (): string => {
    const value = desktopRoot().props.get('--content-unfolded-reserve');
    expect(value, '--content-unfolded-reserve is gone').toBeDefined();
    return value!;
  };

  it('is seven icon boxes and eight gaps', () => {
    expect(unfolded()).toBe('calc(7 * var(--header-icon-box) + 8 * 0.25rem + var(--header-padding-x))');
  });

  it('counts the row\'s OWN gap, like the side reserve', () => {
    const gap = desktopRule(ROW).props.get('gap');
    const term = /\+ 8 \* ([\d.]+rem) \+ var\(--header-padding-x\)\)$/.exec(unfolded())?.[1];
    expect(term, `--content-unfolded-reserve drifted from the row's ${gap} gap`).toBe(gap);
  });

  it('is wider than the side reserve, so growing never undercuts the clamp', () => {
    const boxes = (value: string) => Number(/calc\((\d+) \* var\(--header-icon-box\)/.exec(value)?.[1]);
    const side = desktopRoot().props.get('--content-side-reserve')!;
    expect(boxes(unfolded())).toBeGreaterThan(boxes(side));
  });
});

describe('mobile is untouched', () => {
  it('every rule that positions the title lives in the desktop media query', () => {
    for (const rule of rulesTargeting(shellCss, 'pane-header-content-title')) {
      const positions = ['position', 'left', 'top', 'transform', 'width'].some(p => rule.props.has(p));
      if (!positions) continue;
      expect(rule.atRules, `${rule.selector} escapes the desktop breakpoint`).toBe(DESKTOP);
    }
  });

  it('the mobile sheet still hides it', () => {
    const mobileCss: string = readFileSync(resolve(stylesDir, 'mobile.css'), 'utf-8');
    const hidden = rulesTargeting(mobileCss, 'pane-header-content-title')
      .find(r => r.props.get('display') === 'none');
    expect(hidden, 'the desktop title box would render on top of the mobile row').toBeDefined();
  });
});
