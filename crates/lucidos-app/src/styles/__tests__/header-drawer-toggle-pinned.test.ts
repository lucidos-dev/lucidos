/**
 * The desktop header's drawer toggle rests in ONE place, the header's corner,
 * whether the drawer is open or shut.
 *
 * It used to travel: shut, it sat in the corner; open, it slid to the drawer's
 * far edge and Filter took the corner. A tester looking for it in the corner
 * found Filter there. A control you reach for all day belongs where you left it.
 *
 * What still animates is the collapse exit. Collapsing the whole Conversation
 * pane sends the Canvas pane's hamburger into this corner, so the toggle shrinks
 * away in place rather than fading under it.
 *
 * Source scans: a reintroduced fade or a stray drawer-open position is a source
 * fact. It is cheaper, and more total, to catch in the sheet than in a browser.
 * The rendered half is driven per frame by
 * `e2e/header-drawer-toggle-pinned-desktop.spec.ts`.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting, selectorList, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');
const component = (rel: string): string => readFileSync(resolve(stylesDir, '../components', rel), 'utf-8');

const shellCss = styles('panels/shell.css');
const shellRules = cssRules(shellCss);
const appHeader = component('layout/AppHeader.tsx');

const DESKTOP = '@media (min-width: 769px)';

/** The one desktop rule with this exact selector. */
function desktopRule(selector: string): CssRule {
  const found = shellRules.filter(r => r.selector === selector && r.atRules === DESKTOP);
  expect(found.length, `expected exactly one desktop \`${selector}\` rule`).toBe(1);
  return found[0];
}

describe('one toggle, in one host', () => {
  it('the desktop header mounts exactly one', () => {
    const mounts = appHeader.match(/<ThreadToggleButton\b/g) ?? [];
    expect(mounts.length, 'a second mount is a crossfading pair waiting to happen').toBe(1);
    expect(appHeader).toContain('class="thread-toggle-slot"');
    // The retired second host, in both the markup and the sheet.
    expect(appHeader).not.toContain('thread-nav-group');
    expect(shellCss).not.toContain('thread-nav-group');
  });

  it('the third copy, which could never render on any client, is gone', () => {
    // `.thread-pane-toggle` was mounted by ThreadPane and set `display: none`
    // on desktop AND `display: none !important` on mobile.
    expect(component('layout/ThreadPane.tsx')).not.toContain('ThreadToggleButton');
    for (const rel of ['chat/input-messages.css', 'mobile.css']) {
      expect(styles(rel), `${rel} still hides a toggle that is not rendered`)
        .not.toContain('thread-pane-toggle');
    }
  });

  it('reads in the order it sits on screen: toggle, then Filter, then Search', () => {
    // Tab follows the DOM. The toggle sits ahead of the drawer row on screen,
    // and Filter sits between the title and Search, so the markup says the same.
    const at = (needle: string) => {
      const i = appHeader.indexOf(needle);
      expect(i, `${needle} not found in AppHeader.tsx`).toBeGreaterThanOrEqual(0);
      return i;
    };
    const threadsHeader = at('function ThreadsHeader');
    expect(at('class="thread-toggle-slot"')).toBeLessThan(at('<ThreadsHeader />'));
    const title = appHeader.indexOf('class="threads-header-title"', threadsHeader);
    // Both headers render Filter through one component, which carries its label.
    const filter = appHeader.indexOf('<ThreadFilterButton', threadsHeader);
    const search = appHeader.indexOf('aria-label="Search threads"', threadsHeader);
    expect(title).toBeLessThan(filter);
    expect(filter).toBeLessThan(search);
  });

  it('the host has no focus handler: the button fills it and keeps its clicks', () => {
    expect(appHeader).toContain('<div class="thread-toggle-slot">');
  });
});

describe('it rests in the corner in both drawer states', () => {
  it('no rule gives it a drawer-open position', () => {
    // The drawer-open position was the Conversation pane header's leading
    // edge, and moving there is what put Filter in the corner instead.
    for (const rule of rulesTargeting(shellCss, 'thread-toggle-slot')) {
      expect(rule.selector, `${rule.selector} moves the toggle with the drawer`)
        .not.toContain('data-thread-drawer-open');
    }
    expect(desktopRule('.thread-toggle-slot').props.get('left')).toBe('var(--header-lead-inset)');
  });

  it('no rule fades it, in any state', () => {
    const rules = rulesTargeting(shellCss, 'thread-toggle-slot');
    expect(rules.length, 'the slot lost its rules').toBeGreaterThanOrEqual(2);
    for (const rule of rules) {
      expect(rule.props.get('opacity'), `${rule.selector} { opacity }`).toBeUndefined();
      expect(rule.props.get('transition') ?? '', `${rule.selector} { transition }`)
        .not.toContain('opacity');
    }
  });

  it('the drawer row keeps the toggle\'s box free, and clips at its edge', () => {
    // Arithmetic rather than luck. The row's lead is the toggle's `left`, plus
    // its `width`, plus a gap. The row pads past that and clips at it, so
    // nothing in the row paints or hit-tests under the toggle in any frame.
    // That covers Filter and Search riding the row's right edge into the
    // corner as the drawer shuts.
    const slot = desktopRule('.thread-toggle-slot');
    const row = desktopRule('.threads-header');
    expect(row.props.get('--threads-row-lead')).toBe(
      `calc(${slot.props.get('left')} + ${slot.props.get('width')} + var(--pane-header-gap))`,
    );
    expect(row.props.get('padding')).toMatch(/ var\(--threads-row-lead\)$/);
    expect(row.props.get('clip-path')).toMatch(/ var\(--threads-row-lead\)\)$/);
  });
});

describe('a Conversation-pane collapse shrinks it away', () => {
  it('it leaves with its pane instead of fading where the hamburger lands', () => {
    const rule = desktopRule(':root[data-thread-collapsed] .thread-toggle-slot');
    expect(rule.props.get('width')).toBe('0');
    expect(rule.props.get('visibility')).toBe('hidden');
    expect(rule.props.get('pointer-events')).toBe('none');
  });

  it('`left` is declared once, at the corner, and nothing transitions it', () => {
    // The collapse keeps the corner. Sending the toggle toward the pane's
    // edge instead slid it under the traffic lights on the packaged build.
    const lefts = rulesTargeting(shellCss, 'thread-toggle-slot')
      .filter(r => r.props.has('left'))
      .map(r => `${r.selector} { left: ${r.props.get('left')} }`);
    expect(lefts).toEqual(['.thread-toggle-slot { left: var(--header-lead-inset) }']);
    expect(desktopRule('.thread-toggle-slot').props.get('transition')).not.toMatch(/\bleft\b/);
  });

  it('`width` is what transitions, and a pane resize kills it', () => {
    const base = desktopRule('.thread-toggle-slot');
    expect(base.props.get('position')).toBe('absolute');
    // `width` carries the exit; `auto` is not interpolable, so the slot states
    // its own width for that transition to have a from-value at all.
    expect(base.props.get('transition')).toContain('width var(--duration-slow) ease');
    // visibility rides along for the tab order. It steps at the END of its
    // duration, so the button stays paintable while it shrinks.
    expect(base.props.get('transition')).toContain('visibility var(--duration-slow)');
    // A drag on the split divider can re-expand a collapsed pane, which regrows
    // the toggle's width. Out of the kill list it would ease in behind a pane
    // that is tracking the pointer 1:1.
    const killed = shellRules.find(
      r => r.atRules === DESKTOP && r.props.get('transition') === 'none'
        && r.selector.includes('[data-pane-resizing]'),
    );
    expect(killed?.selector, 'the pane-resize kill list is gone')
      .toContain(':root[data-pane-resizing] .thread-toggle-slot');
  });

  it('the slot clips at one constant edge, clear of the focus ring', () => {
    // The clip is what a shrinking slot paints through, so it must hold in
    // every frame. `overflow: clip` cannot: it does not transition, so it
    // lifted on the first frame of a re-expand and flashed the whole button
    // over the Canvas hamburger. A clip-path declared once, on the base rule,
    // has nothing to drop.
    const clip = desktopRule('.thread-toggle-slot').props.get('clip-path') ?? '';
    const slack = -parseFloat(/^inset\((-[\d.]+)rem\)$/.exec(clip)?.[1] ?? 'NaN');
    // The header's own ring band, read where the header recolours it.
    const ring = shellRules.find(r => r.selector === '.app-header .icon-btn:focus-visible');
    const band = parseFloat(/0 0 0 ([\d.]+)rem/.exec(ring?.props.get('--focus-ring') ?? '')?.[1] ?? 'NaN');
    expect(band, 'the header focus ring is no longer a rem band').toBeGreaterThan(0);
    expect(slack, `clip ${clip} cuts the ${band}rem focus ring`).toBeGreaterThanOrEqual(band);
    for (const rule of rulesTargeting(shellCss, 'thread-toggle-slot')) {
      expect(rule.props.get('overflow'), `${rule.selector} { overflow }`).toBeUndefined();
      if (rule.selector === '.thread-toggle-slot') continue;
      expect(rule.props.get('clip-path'), `${rule.selector} swaps the clip`).toBeUndefined();
    }
  });

  it('the whole-region collapse fade does not reach it', () => {
    // The regions (.threads-header, .pane-header-brand, .content-header-elements)
    // still fade as their pane leaves: each clips to zero width against a
    // neighbour that is adjacent by construction. The toggle is a single
    // control in the corner the hamburger arrives in, which is exactly why it
    // must not fade there.
    for (const rule of shellRules) {
      if (rule.props.get('opacity') !== '0') continue;
      expect(rule.selector, `${rule.selector} { opacity: 0 }`).not.toContain('thread-toggle-slot');
    }
  });

  it('the slot\'s stated width is the button\'s own box, in both axes', () => {
    // --header-icon-box is pinned to the button's HEIGHT in
    // header-band-centering.test.ts. The slot declares it as a WIDTH, so the
    // button has to be square or the resting slot clips its own icon.
    // By selector-list MEMBER, not by the whole selector text. The box is
    // declared on a rule the button shares with `.icon-btn.row-icon`. An exact
    // compare lands on the band's nominal rule instead, reads two `undefined`s,
    // and passes having asserted nothing.
    const iconBox = cssRules(styles('global/host-components.css'))
      .find(r => selectorList(r.selector).includes('.icon-btn.header-icon') && r.props.has('width'));
    expect(iconBox, 'no .icon-btn.header-icon rule declaring a width').toBeDefined();
    expect(iconBox!.props.get('width'), 'the header icon button is no longer square')
      .toBe(iconBox!.props.get('height'));
    expect(desktopRule('.thread-toggle-slot').props.get('width')).toBe('var(--header-icon-box)');
  });

  it('the button does not shrink with the slot, or it would squash instead of clip', () => {
    // The exit clips a full-size button behind a shrinking box. A flex child
    // with the default `flex-shrink: 1` would be squeezed to nothing instead.
    // The icon would then deform on its way out rather than being clipped.
    const iconBtn = cssRules(styles('global/shared-components.css'))
      .find(r => r.selector === '.icon-btn');
    expect(iconBtn?.props.get('flex-shrink')).toBe('0');
  });
});

describe('the brand region survives losing its leading child', () => {
  it('it pins its actions itself, now that nothing leads its flow', () => {
    // `space-between` meant "trailing edge" only while the toggle was the other
    // in-flow child. With the toggle out of the flow, space-between would put
    // the actions cluster at the LEADING edge, under the toggle.
    expect(desktopRule('.app-header .pane-header-brand').props.get('justify-content'))
      .toBe('flex-end');
  });

  it('the action-collapse measurement names no leading zone here', () => {
    // The measurement derives the leading width from the container and the
    // centred box. It measures no element, so the thread row needs no
    // `leading` selector. One pointing at a retired element would be a dead
    // selector that reads as live config.
    expect(component('layout/ThreadHeaderActions.tsx')).not.toContain('leading:');
  });
});
