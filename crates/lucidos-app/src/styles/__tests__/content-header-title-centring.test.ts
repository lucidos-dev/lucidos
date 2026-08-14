/**
 * Source scans over the desktop content row's title cluster: it is CENTRED on
 * the row, and the room it keeps clear at each end is DERIVED from the controls
 * that stand there rather than guessed.
 *
 * The bug this pins, reported 2026-08-13: the title and both chevrons moved
 * whenever the trailing action cluster changed width, most visibly as the ⋯
 * overflow trigger came and went. They were the `flex: 1` middle of a 3-zone
 * row, so their position was the midpoint between the hamburger and a cluster
 * that is 1 to 3 icon boxes wide depending on the content view.
 *
 * Two things need pinning here rather than in a browser spec, and one thing
 * does not:
 *
 * 1. The reserve's ARITHMETIC. `e2e/content-title-position-desktop.spec.ts`
 *    proves the chevrons hold still and clear the cluster at the widths it
 *    drives, which is what the user sees. It cannot see WHY: that the reserve
 *    is three `--header-icon-box`es and four gaps because that is the widest
 *    cluster `useHeaderActionCollapse` can leave standing plus what its fit
 *    model charges. A reserve re-eyeballed to a rem constant would pass the
 *    e2e at 100% ui-scale and put an icon on a chevron at 150%.
 * 2. The COPIES. CSS cannot hand a rule's own declaration back, so the gap
 *    inside the reserve is a copy of the row's own `gap`, and the two span
 *    tokens are shared with the thread pane's clamp. Same shape as the drift
 *    checks in header-band-centering.test.ts, which holds --threads-title-lead
 *    to the threads row's padding and --header-icon-box to the button.
 *
 * The vertical half is NOT here: the row's bar centring, and its packaged-macOS
 * `left` floor, belong to header-band-centering.test.ts and are untouched by
 * this arrangement (every rule below is horizontal).
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const shellCss: string = readFileSync(resolve(stylesDir, 'panels/shell.css'), 'utf-8');
const shellRules = cssRules(shellCss);

const DESKTOP = '@media (min-width: 769px)';

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
    // `flex: 1` is what centred it on the GAP between the two clusters, and
    // `min-width: 0` was that arrangement's non-overlap guarantee. Both are the
    // old model; the clamp below is the new one.
    expect(title.props.get('flex'), 'flex is what centred it on the gap').toBeUndefined();
    expect(title.props.get('min-width'), 'the clamp is the guarantee now').toBeUndefined();
  });

  it('leaves the hamburger and the actions pinned to the row\'s two ends', () => {
    // The title's `flex: 1` used to push the actions to the trailing edge. With
    // it out of flow the row has two in-flow children left, and this is what
    // holds them apart, exactly as `.threads-header` holds its own pair.
    expect(desktopRule(ROW).props.get('justify-content')).toBe('space-between');
    const ends = desktopRule(
      '.app-header .content-header-elements > .hamburger-panel, .app-header .content-header-actions',
    );
    expect(ends.props.get('flex-shrink'), 'only the title may give way').toBe('0');
  });

  it('clears the WIDER of the row\'s two ends on BOTH sides', () => {
    // A box centred on the row overlaps a cluster as soon as it is wider than
    // twice the distance from the row's middle to that cluster's inner edge, so
    // the clamp IS the structural non-overlap guarantee. Same three-part shape
    // as the thread pane's brand label: hold the span when the pane can afford
    // it, give way to the reserves when it cannot, never squeeze below the
    // controls' own width.
    expect(desktopRule(TITLE).props.get('width')).toBe(
      'clamp( var(--desktop-nav-min-span), calc(100% - 2 * var(--content-side-reserve)),'
      + ' var(--desktop-nav-span) )',
    );
  });

  it('holds the chevrons the same distance apart as the thread pane\'s', () => {
    // One span, two panes. The tokens are shared rather than restated, so a
    // retune moves both rows' chevrons together instead of splitting them.
    const brand = desktopRule('.app-header .pane-header-brand .pane-header-brand-label');
    for (const token of ['--desktop-nav-min-span', '--desktop-nav-span']) {
      expect(brand.props.get('width'), `${token} is not shared`).toContain(token);
    }
  });

  it('states no height of its own, so it cannot hang out of the row', () => {
    // The box is out of flow, so it no longer contributes to the row's height:
    // the row is as tall as the hamburger, and this box as tall as a chevron.
    // Both are `.icon-btn.header-icon`, so they agree by construction, and a
    // height stated HERE is the way that agreement gets broken. It matters
    // because the row's `overflow: clip` behaves as a SCROLL container on the
    // packaged macOS webview, and a box hanging out of it vertically is
    // scrolled away and back on every click inside it (the defect
    // .pane-header-brand's min-height note describes).
    const sized = rulesTargeting(shellCss, 'pane-header-content-title')
      .filter(r => r.props.has('height') || r.props.has('min-height'));
    expect(sized, 'the title box states a height of its own').toEqual([]);
  });

  it('keeps the trailing cluster painting above it', () => {
    // A positioned element paints after its in-flow siblings, so the box would
    // cover any icon it reached and eat the click. The mobile row carries the
    // same guard for the same reason.
    const actions = desktopRule('.app-header .content-header-actions');
    expect(actions.props.get('position')).toBe('relative');
    expect(actions.props.get('z-index')).toBe('1');
  });

  it('a chevron on the clip edge draws its focus ring INSET', () => {
    // The box clips at its own edges and `space-between` pins a chevron to
    // each, so the shared outward --focus-ring would lose its outer side. Both
    // centred rows carry the same construct, so the rule covers both; a fix on
    // one only is the drift this whole change removes.
    const ring = desktopRule(
      `${TITLE} > .icon-btn:focus-visible, .app-header .pane-header-brand-label > .icon-btn:focus-visible`,
    );
    expect(ring.props.get('box-shadow')).toBe('inset var(--focus-ring)');
    // Forced-colors strips box-shadow, leaving the transparent outline as the
    // whole ring, so it has to come inward by the same amount.
    expect(ring.props.get('outline-offset'), 'the forced-colors fallback still rings outward')
      .toBe('-0.125rem');
  });

  it('the ring band fits inside the button, clear of the glyph', () => {
    // Why inset is affordable here at all: the band is drawn in the padding
    // between the button's box and its ink. Read from the tokens rather than
    // asserted as a number, so a retune of either is what fails this.
    const iconBox = parseFloat(desktopRoot().props.get('--header-icon-box')!);
    const base = readFileSync(resolve(stylesDir, 'global/base.css'), 'utf-8');
    const glyph = parseFloat(/--icon-size-lg:\s*([\d.]+)rem/.exec(base)![1]);
    const band = parseFloat(/0 0 0 ([\d.]+)rem/.exec(/--focus-ring:([^;]+);/.exec(base)![1])![1]);
    expect((iconBox - glyph) / 2, 'the inset band would paint over the glyph')
      .toBeGreaterThan(band);
  });

  it('the chevrons are DIRECT children, so the rule holding their size applies', () => {
    // They spent a while inside a `.header-title-span` that carried the span the
    // box carries now, and for all of it this direct-child rule addressed
    // nothing, leaving a long title free to squeeze a chevron.
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

  it('is three icon boxes and four gaps', () => {
    // Three: two context icons riding the row plus the bell, which is the
    // widest the cluster gets (a set of three or more folds whole, see
    // alwaysCollapseFrom, so past two icons it gets NARROWER). Four gaps: the
    // two between those boxes, and the two the collapse measurement charges the
    // centred box, one at each of its sides. Pinned as arithmetic on the icon
    // box rather than as a length, because the lights reserve's own note
    // applies here too in reverse: everything in this row is rem, so a px or
    // eyeballed constant is right at exactly one ui-scale.
    expect(reserve()).toBe('calc(3 * var(--header-icon-box) + 4 * 0.25rem)');
  });

  it('counts the row\'s OWN gap, not a number that once matched it', () => {
    // A copy CSS cannot check for itself: the gap term is a literal because
    // --pane-header-gap is declared on .pane-header and a custom property is
    // substituted on the element that declares it, so :root cannot reference
    // it (the same reason --brand-side-reserve spells it out).
    const gap = desktopRule(ROW).props.get('gap');
    expect(gap, 'the row declares no gap').toBeDefined();
    const term = /\+ 4 \* ([\d.]+rem)\)$/.exec(reserve())?.[1];
    expect(term, `--content-side-reserve drifted from the row's ${gap} gap`).toBe(gap);
  });

  it('is at least the widest cluster the collapse can leave standing', () => {
    // The claim the whole arrangement rests on, checked as numbers at the
    // default 16px root: 3 boxes + 2 gaps of cluster, and the 2 gaps the fit
    // model adds, against the reserve. The unit suite
    // (hooks/useHeaderActionCollapse.test.ts) walks the same arithmetic from
    // the measurement's side; this end is the stylesheet's.
    const iconBox = parseFloat(desktopRoot().props.get('--header-icon-box')!);
    const gapRem = parseFloat(desktopRule(ROW).props.get('gap')!);
    const widestCluster = 3 * iconBox + 2 * gapRem;
    const boxes = Number(/calc\((\d+) \* var\(--header-icon-box\)/.exec(reserve())?.[1]);
    const gaps = Number(/\+ (\d+) \* [\d.]+rem\)$/.exec(reserve())?.[1]);
    expect(boxes * iconBox + gaps * gapRem).toBeGreaterThanOrEqual(widestCluster + 2 * gapRem);
  });
});

describe('mobile is untouched', () => {
  it('every rule that positions the title lives in the desktop media query', () => {
    // The mobile content row renders its own centred cluster
    // (.header-nav-cluster, styles/header-mark.css) and hides this box outright.
    // A positioning rule escaping the breakpoint would fight both.
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
