/**
 * The desktop header's drawer toggle is ONE element that travels, never two
 * that crossfade.
 *
 * The bug this pins: the toggle existed TWICE, once pinned at the header's
 * leading edge (`.collapsed-thread-actions`) and once inside
 * `.pane-header-brand` (`.thread-nav-group`), and the pair crossfaded on
 * `data-thread-drawer-open`. Minimizing the drawer therefore faded an icon UP
 * from nothing at a position it had never occupied, while its twin slid toward
 * it shrinking and fading DOWN, so for most of the slide the header carried two
 * half-transparent icons that ended on nearly the same x. The user's words:
 * "it must not be placed behind and gradually come to life", "no dimming or
 * icons on top of each other".
 *
 * The same defect had a second instance one animation over. Collapsing the
 * whole Conversation pane faded the toggle out IN PLACE at its resting inset,
 * which is precisely the slot the Canvas pane's hamburger slides left into, so
 * a dimming ghost sat under an arriving control.
 *
 * Split out of `header-band-centering.test.ts` rather than added to it: that
 * file scans because none of what it covers reproduces in a rendered frame off
 * the packaged macOS build, which is not true here. This one is a source scan
 * for a different reason. A reintroduced fade is a source fact and is cheaper
 * (and more total) to catch in the sheet than to sample in a browser, and the
 * non-overlap guarantee is arithmetic over two declarations rather than a
 * measurement. The moving half IS driven in a browser, mid-animation, by
 * `e2e/header-drawer-toggle-travel-desktop.spec.ts`.
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

const shellCss = styles('panels/shell.css');
const shellRules = cssRules(shellCss);

const DESKTOP = '@media (min-width: 769px)';

/** The one desktop rule with this exact selector. */
function desktopRule(selector: string): CssRule {
  const found = shellRules.filter(r => r.selector === selector && r.atRules === DESKTOP);
  expect(found.length, `expected exactly one desktop \`${selector}\` rule`).toBe(1);
  return found[0];
}

describe('one toggle, not a crossfading pair', () => {
  it('the desktop header mounts exactly one', () => {
    const appHeader = readFileSync(
      resolve(stylesDir, '../components/layout/AppHeader.tsx'), 'utf-8',
    );
    const mounts = appHeader.match(/<ThreadToggleButton\b/g) ?? [];
    expect(mounts.length, 'a second mount is a crossfading pair waiting to happen').toBe(1);
    expect(appHeader).toContain('class="thread-toggle-slot"');
    // The retired second host, in both the markup and the sheet. Its CSS is
    // gone, so a stray mount would land unstyled in the brand's flow rather
    // than announcing itself.
    expect(appHeader).not.toContain('thread-nav-group');
    expect(shellCss).not.toContain('thread-nav-group');
  });

  it('the third copy, which could never render on any client, is gone', () => {
    // `.thread-pane-toggle` was mounted by ThreadPane and set `display: none`
    // on desktop AND `display: none !important` on mobile.
    const threadPane = readFileSync(
      resolve(stylesDir, '../components/layout/ThreadPane.tsx'), 'utf-8',
    );
    expect(threadPane).not.toContain('ThreadToggleButton');
    for (const rel of ['chat/input-messages.css', 'mobile.css']) {
      expect(styles(rel), `${rel} still hides a toggle that is not rendered`)
        .not.toContain('thread-pane-toggle');
    }
  });
});

describe('it travels; nothing dims', () => {
  it('no rule fades it, in any state', () => {
    const rules = rulesTargeting(shellCss, 'thread-toggle-slot');
    expect(rules.length, 'the slot lost its rules').toBeGreaterThanOrEqual(3);
    for (const rule of rules) {
      expect(rule.props.get('opacity'), `${rule.selector} { opacity }`).toBeUndefined();
      expect(rule.props.get('transition') ?? '', `${rule.selector} { transition }`)
        .not.toContain('opacity');
    }
  });

  it('`left` is what transitions, so the icon moves instead of swapping', () => {
    const base = desktopRule('.thread-toggle-slot');
    expect(base.props.get('position')).toBe('absolute');
    expect(base.props.get('transition')).toContain('left var(--duration-slow) ease');
    // `width` carries the collapse exit (see the suite below); `auto` is not
    // interpolable, so the slot states its own width for that transition to
    // have a from-value at all.
    expect(base.props.get('transition')).toContain('width var(--duration-slow) ease');
    expect(base.props.get('width')).toBe('var(--header-icon-box)');
    // visibility rides along for the tab order on a pane collapse. It steps at
    // the END of its duration, so the button stays paintable while it travels.
    expect(base.props.get('transition')).toContain('visibility var(--duration-slow)');
  });

  it('…which is exactly why it belongs in the pane-resize kill list', () => {
    // The cost of the transition, and it is not optional: the slot's `left`
    // depends on --co, and a drawer-divider drag rewrites --co on every
    // pointermove (DrawerDivider, via beginPaneResize). Left out of this block
    // the toggle is the one control in the bar still easing while the drawer,
    // the row beside it and the brand region it leads all track the pointer
    // 1:1 -- a 300ms lag for the whole drag. It could not lag before this
    // change, having had a constant `left` and no geometry transition at all,
    // which is exactly the trap `.claude/rules/frontend.md` names: a NEW
    // travelling header element has to join the list in the same change.
    const killed = shellRules.find(
      r => r.atRules === DESKTOP && r.props.get('transition') === 'none'
        && r.selector.includes('[data-pane-resizing]'),
    );
    expect(killed?.selector, 'the pane-resize kill list is gone')
      .toContain(':root[data-pane-resizing] .thread-toggle-slot');
  });

  it('drawer open, it sits at the Conversation pane header\'s leading edge', () => {
    // The exact x the retired second copy occupied as the brand's first in-flow
    // child, so the settled layout did not move. One rule for both desktop
    // builds: the drawer's own floor is derived from a row that starts after
    // the traffic-lights reserve (store/paneMinimums.ts, ADR 0058), so this is
    // always well clear of the lights and needs no overlay-build variant.
    expect(desktopRule(':root[data-thread-drawer-open] .thread-toggle-slot').props.get('left'))
      .toBe('calc(var(--co) + var(--ddo))');
  });

  it('it clears the drawer row for the WHOLE slide, not just at the two ends', () => {
    // Arithmetic rather than luck. The row's right edge IS its `width`; the
    // toggle's left is that same term plus the drawer divider. Both interpolate
    // over the same var(--duration-slow) ease, from (--co, --co + --ddo) to
    // (0, the home inset), and the toggle is the greater at both ends, so it is
    // the greater at every point between. The two are separate copies of --co
    // that CSS cannot hand back as a resolved length, which is what makes this
    // drift check the thing standing behind that argument.
    const rowWidth = desktopRule('.threads-header').props.get('width');
    expect(rowWidth).toBe('var(--co)');
    expect(desktopRule(':root[data-thread-drawer-open] .thread-toggle-slot').props.get('left'))
      .toBe(`calc(${rowWidth} + var(--ddo))`);
    // The inset the travel lands on is the row's own padding, i.e. the sliver
    // that row bottoms out at, so the arrival point is clear of it too. It is
    // declared on :root because --brand-side-reserve reads the same value; see
    // header-band-centering.test.ts for that half.
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    expect(root?.props.get('--brand-lead-inset'))
      .toBe(desktopRule('.threads-header').props.get('padding')!.split(' ')[1]);
  });
});

describe('a Conversation-pane collapse shrinks it away on the same track', () => {
  it('it leaves with its pane instead of fading where the hamburger lands', () => {
    const rule = desktopRule(':root[data-thread-collapsed] .thread-toggle-slot');
    expect(rule.props.get('width')).toBe('0');
    expect(rule.props.get('visibility')).toBe('hidden');
    expect(rule.props.get('pointer-events')).toBe('none');
  });

  it('`left` NEVER leaves the --co + --ddo track, in any state', () => {
    // The regression this pins, found in review. A negative `left` was the
    // obvious way to send the toggle off the leading edge, and it broke the way
    // BACK: `left` interpolates between two resolved lengths, so a toggle parked
    // at -1 icon box re-enters a whole box LEFT of where the drawer row's right
    // edge starts, and spends 92% of a re-expand inside the growing row, over
    // its Search button. Every state's `left` must therefore be the track or the
    // home inset, both of which are >= the row's right edge by construction.
    const TRACK = 'calc(var(--co) + var(--ddo))';
    const allowed = new Set([TRACK, 'var(--brand-lead-inset)']);
    const lefts = rulesTargeting(shellCss, 'thread-toggle-slot')
      .map(r => r.props.get('left'))
      .filter((v): v is string => v !== undefined);
    expect(lefts.length, 'no rule positions the slot any more').toBeGreaterThanOrEqual(2);
    for (const left of lefts) {
      expect(allowed.has(left), `off-track \`left: ${left}\``).toBe(true);
    }
    // …and the collapsed state is specifically ON the track, not at the home
    // inset, so the shrink happens where the pane's edge actually is.
    expect(desktopRule(':root[data-thread-collapsed] .thread-toggle-slot').props.get('left'))
      .toBe(TRACK);
  });

  it('the clip that hides the shrink is scoped to the one state that cannot hold focus', () => {
    // `overflow: clip` on the slot clips the button's focus-ring box-shadow with
    // it. That is free HERE and only here, because the collapsed state is also
    // `visibility: hidden`, so the button is not focusable and cannot be wearing
    // a ring. On the base rule it would clip a real one.
    expect(desktopRule(':root[data-thread-collapsed] .thread-toggle-slot').props.get('overflow'))
      .toBe('clip');
    for (const rule of rulesTargeting(shellCss, 'thread-toggle-slot')) {
      if (rule.selector.includes('[data-thread-collapsed]')) continue;
      expect(rule.props.get('overflow'), `${rule.selector} { overflow } clips the focus ring`)
        .toBeUndefined();
    }
  });

  it('the whole-region collapse fade does not reach it', () => {
    // The regions (.threads-header, .pane-header-brand, .content-header-elements)
    // still fade as their pane leaves: each clips to zero width against a
    // neighbour that is adjacent by construction, so none of them can end up
    // under another. The toggle is a single control pinned to a constant that
    // tracks nothing, which is exactly why it could, and why it is not in the
    // list.
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
    // with the default `flex-shrink: 1` would instead be squeezed to nothing,
    // so the icon would deform on its way out rather than being clipped.
    const iconBtn = cssRules(styles('global/shared-components.css'))
      .find(r => r.selector === '.icon-btn');
    expect(iconBtn?.props.get('flex-shrink')).toBe('0');
  });
});

describe('the brand region survives losing its leading child', () => {
  it('it pins its actions itself, now that nothing leads its flow', () => {
    // `space-between` meant "trailing edge" only while the toggle was the other
    // in-flow child. With the toggle out of the flow, space-between would put
    // the actions cluster at the LEADING edge, under the travelling toggle.
    expect(desktopRule('.app-header .pane-header-brand').props.get('justify-content'))
      .toBe('flex-end');
  });

  it('the action-collapse measurement names no leading zone here', () => {
    // The measurement derives the leading width from the container and the
    // centred box instead of measuring an element, so the thread row's
    // `leading` selector was already unread. Leaving it pointing at the deleted
    // `.thread-nav-group` would be a dead selector that reads as live config.
    // (It used to say so with a `centred: true` flag beside it. The content row
    // is centred too since 2026-08-13, so there is no other mode left to
    // distinguish and the flag went with it.)
    const targets = readFileSync(
      resolve(stylesDir, '../components/layout/ThreadHeaderActions.tsx'), 'utf-8',
    );
    expect(targets).not.toContain('leading:');
  });
});
