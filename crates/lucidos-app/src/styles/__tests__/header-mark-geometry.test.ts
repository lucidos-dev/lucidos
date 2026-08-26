/**
 * Source scans over the mobile header's structural promises, plus the one the
 * desktop brand span repeats verbatim. Each is a property of the STYLESHEET
 * rather than of a rendered frame: the e2e specs measure the painted result,
 * this pins the rule that produces it.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, cssRules, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');

const markCss = styles('header-mark.css');
const mobileCss = styles('mobile.css');
const shellCss = styles('panels/shell.css');
// The switcher's markup, for the one promise here that spans both: which box
// the Manage workspaces row is nested in decides whether it can scroll away.
const switcherSource: string = readFileSync(
  resolve(here, '../../components/layout/WorkspaceSwitcher.tsx'),
  'utf-8',
);

/** The menu carries the Restart control, and the toast most likely to be on
 *  screen when a user reaches for it is the persistent "Restart needed" one.
 *  `--z-modal` (2300) sits deliberately BELOW `--z-toast` (2400), so the
 *  ordinary modal layer would put the toast over the very control the menu was
 *  opened for.
 *
 *  Asserted on the DECLARATION rather than on a computed number, because the
 *  failure mode is someone "tidying" these to `var(--z-modal)`, which no
 *  arithmetic over the tokens would catch. */
describe('the menu paints above a persistent toast', () => {
  it('the panel and its scrim are rebased above --z-toast, panel over scrim', () => {
    expect(decl(block(markCss, '.brand-menu {'), 'z-index')).toBe('calc(var(--z-toast) + 50)');
    expect(decl(block(markCss, '.brand-menu-scrim {'), 'z-index')).toBe('calc(var(--z-toast) + 40)');
  });
});

/** The unfolded workspace list is the one thing in the panel whose height the
 *  panel does not control: it is as long as the machine has workspaces. The
 *  panel is `position: fixed` under the header and `overflow: hidden`, so an
 *  uncapped list is CLIPPED rather than scrolled and the rows past the cut
 *  cannot be reached at all. Both halves are required and each is useless
 *  alone: a cap with no scroll hides the tail, a scroll with no cap never
 *  engages. */
describe('the unfolded workspace list scrolls instead of overflowing the panel', () => {
  it('.brand-menu-ws-scroll caps its height and scrolls', () => {
    const scroll = block(markCss, '.brand-menu-ws-scroll {');
    expect(decl(scroll, 'max-height')).toBe('var(--brand-menu-ws-list-max-height)');
    expect(decl(scroll, 'overflow-y')).toBe('auto');
  });

  it('Manage workspaces sits outside the scroller, so it never scrolls away', () => {
    // It is the way OUT of a list too long to read. Inside the scroller it was
    // the one row a long list hid, which is the report this answers.
    const list = block(markCss, '.brand-menu-ws-list {');
    expect(decl(list, 'max-height')).toBeNull();
    expect(decl(list, 'overflow-y')).toBeNull();

    // The markup half: the manage row is a sibling of the scroller, so every
    // `<div>` opened after it is closed again before the row.
    const from = switcherSource.indexOf('<div class="brand-menu-ws-scroll">');
    const to = switcherSource.indexOf('brand-menu-ws-row brand-menu-ws-manage', from);
    expect(from, 'the scroller is gone').toBeGreaterThanOrEqual(0);
    expect(to, 'the manage row is gone').toBeGreaterThan(from);
    const between = switcherSource.slice(from, to);
    expect(between.match(/<\/div>/g)?.length).toBe(between.match(/<div\b/g)?.length);
  });

  it('the panel itself is bounded by the room below the header', () => {
    // The floor under the list's own cap: the panel is `position: fixed`, so a
    // phone in landscape cannot reach its last rows. Horizontal stays clipped,
    // since the rounded corners are what clip the rows' hover backgrounds.
    const panel = block(markCss, '.brand-menu {');
    expect(decl(panel, 'max-height')).toContain('var(--app-header-bottom)');
    expect(decl(panel, 'overflow')).toBe('hidden auto');
  });

  it('the cap is bounded by the viewport, not by rem alone', () => {
    // A rem-only cap is a fixed number of rows, which still overruns a phone in
    // landscape (and any scaled root), where the panel has far less room below
    // the header than a desktop window does.
    const cap = decl(block(markCss, ':root {'), '--brand-menu-ws-list-max-height');
    expect(cap).toContain('vh');
  });
});

/** The action a right-click unfolds hangs under the row it belongs to. Its
 *  glyph has to start on that row's NAME column, or it reads as a sibling
 *  instead of a satellite. Two hosts unfold one, and their rows lead with
 *  different columns, so there are two indents and neither may be a constant:
 *  each is the row's own padding plus its own leading box plus its own gap. */
describe('an unfolded action indents off the row it hangs under', () => {
  it('leaves the shared rule with the dressing and no indent of its own', () => {
    const shared = block(markCss, '.brand-menu-ws-action {');
    expect(decl(shared, 'color')).toBe('var(--text-muted)');
    expect(
      decl(shared, 'padding-left'),
      'an indent here would be one host\'s column imposed on both',
    ).toBeNull();
  });

  it('derives each indent from its own host\'s leading column', () => {
    // Under a switcher row, which leads with a status dot.
    const underDot = decl(block(markCss, '.brand-menu-ws-action-under-dot {'), 'padding-left') ?? '';
    expect(underDot).toContain('var(--ws-picker-dot-size)');
    // Under a notifications row, a `.brand-menu-item` leading with the bell.
    const underIcon = decl(block(markCss, '.brand-menu-ws-action-under-icon {'), 'padding-left') ?? '';
    expect(underIcon).toContain('var(--icon-size-md)');
    // Both against the spacing scale, never an eyeballed rem.
    for (const indent of [underDot, underIcon]) {
      expect(indent, 'a raw rem agrees with the row above it at one ui-scale only')
        .toMatch(/var\(--space-(xs|sm|md|lg|xl)\)/);
      expect(indent).not.toMatch(/\d+(\.\d+)?rem/);
    }
  });
});

/** The notice the panel leads with while the mark is dim. Its PRESENCE is
 *  pinned in `components/layout/__tests__/connection-notice.test.ts`. Only the
 *  stylesheet can say that it reads as a notice rather than as a row, and that
 *  its dot is drawn at all. */
describe('the connection notice is a raised statement, not a row', () => {
  const notice = block(markCss, '.brand-menu-notice {');

  it('lifts on the shared surface, with no frame and no stripe', () => {
    expect(decl(notice, 'background')).toBe('var(--bg-tertiary)');
    // A coloured left edge on a callout is banned outright
    // (.claude/rules/frontend-css.md), and a full border would make the panel
    // read as two panels, which is what the separator below it already avoids.
    for (const frame of ['border', 'border-left', 'box-shadow', 'outline']) {
      expect(decl(notice, frame), `a ${frame} turns the notice into a card`).toBeNull();
    }
    // Concentric with the panel's own corners, like every row in it.
    expect(decl(notice, 'border-radius')).toBe('var(--brand-menu-item-radius)');
  });

  it('takes nothing that would promise a tap', () => {
    const targeting = rulesTargeting(markCss, 'brand-menu-notice');
    for (const rule of targeting) {
      expect(rule.props.get('cursor'), `${rule.selector} offers a pointer`).toBeUndefined();
      expect(rule.selector, 'a hover state would make it look like a control')
        .not.toContain(':hover');
    }
  });

  it('lands its dot on the first line, derived rather than nudged', () => {
    // The text wraps in a fixed-width panel, so centring the dot against the
    // whole box floats it into the middle of a sentence. Both quantities the
    // offset is made of have to be the ones actually in force, or the dot
    // drifts off the line the moment either is retuned.
    expect(decl(notice, 'align-items')).toBe('flex-start');
    const dot = block(markCss, '.brand-menu-notice .status-dot {');
    const offset = decl(dot, 'margin-top') ?? '';
    expect(offset).toContain('var(--brand-menu-row-line-height)');
    expect(offset).toContain('var(--brand-menu-notice-dot)');
    expect(decl(notice, 'line-height')).toBe('var(--brand-menu-row-line-height)');
    expect(decl(dot, 'width')).toBe('var(--brand-menu-notice-dot)');
  });

  it('gets its colour from the shared dot scale, for every state it can show', () => {
    // The notice names the state in its class list and lets `.status-dot`
    // colour it, so the two must actually meet: a state with no step in that
    // scale draws the muted fallback and says nothing.
    const dotStates = rulesTargeting(shellCss, 'status-dot')
      .flatMap(r => [...r.selector.matchAll(/\.status-dot\.([\w-]+)/g)].map(m => m[1]));
    for (const state of ['connecting', 'disconnected']) {
      expect(dotStates, `.status-dot.${state} has no colour of its own`).toContain(state);
    }
  });
});

describe('mobile header rows share one height', () => {
  it('.mobile-header-row sets a fixed height, not a min-height', () => {
    const row = block(mobileCss, '.mobile-header-row {');
    expect(decl(row, 'height')).toBe('var(--mobile-header-row-height)');
    expect(
      decl(row, 'min-height'),
      'a min-height makes each row as tall as its own tallest control, which is the bug',
    ).toBeNull();
  });

  it('the height token is the tallest control the rows carry', () => {
    // The mark's tap target is the tallest thing on any of the three rows, so
    // sizing off it is what lets them agree without anything shrinking.
    const mobileRoot = block(mobileCss, ':root {');
    expect(decl(mobileRoot, '--mobile-header-row-height')).toBe('var(--header-mark-tap)');
    expect(decl(block(markCss, ':root {'), '--header-mark-tap')).not.toBeNull();
  });

  it('nothing re-introduces a per-row height override', () => {
    const overrides = rulesTargeting(mobileCss, 'mobile-header-row')
      .filter(r => r.props.has('height') || r.props.has('min-height'))
      .filter(r => r.selector !== '.mobile-header-row');
    expect(
      overrides.map(r => `${r.atRules} ${r.selector}`),
      'a height override on one row is the same bug in a new place',
    ).toEqual([]);
  });
});

describe('the mark says its connection in strength alone', () => {
  it('connected is the header foreground at full light, and nothing else', () => {
    const connected = block(markCss, '.brand-mark[data-conn="connected"] .brand-mark-glyph {');
    expect(
      decl(connected, 'color'),
      'the state the user looks at all day is the brand at full strength',
    ).toBe('var(--header-fg)');
    // Everything a frame around the mark could be called instead.
    for (const decoration of ['background', 'border', 'box-shadow', 'outline', 'animation', 'opacity']) {
      expect(decl(connected, decoration), `connected must carry no ${decoration}`).toBeNull();
    }
  });

  it('has no ring pseudo-element on the tile', () => {
    expect(markCss).not.toMatch(/\.brand-mark-glyph::(after|before)/);
  });

  it('states the two receded states as opacity only', () => {
    const disconnected = block(markCss, '.brand-mark[data-conn="disconnected"] .brand-mark-glyph {');
    expect(decl(disconnected, 'opacity')).toBe('var(--header-mark-disconnected-opacity)');
    // No colour of its own: a receded state falls back to the muted base every
    // other glyph in the bar wears, so the ladder stays one-dimensional. The
    // readable half lives in the aria-label (BrandMenuButton).
    expect(decl(disconnected, 'color')).toBeNull();
    expect(decl(disconnected, 'background')).toBeNull();

    const connecting = block(markCss, '.brand-mark[data-conn="connecting"] .brand-mark-glyph {');
    expect(decl(connecting, 'animation')).toContain('brand-mark-breathe');
    expect(decl(connecting, 'color')).toBeNull();
  });

  it('reduced motion actually beats the animation it is cancelling', () => {
    // The cascade half. A media query adds no specificity, so the override has
    // to reach the animated rule's own specificity to win. Asserting that the
    // reduce block targets the SAME selector is the durable way to say that: a
    // broader one would parse fine, read fine, and never apply.
    //
    // Scoped to the MARK's own rules, not to every rule in the file. The
    // stylesheet also dresses the menu, whose panel and scrim carry the
    // one-shot `modal-in` entrance every overlay uses. This test is about the
    // mark's CONNECTION states, where a looping animation is the signal and
    // cancelling it has to leave the state legible.
    const markRules = cssRules(markCss).filter(r => r.selector.startsWith('.brand-mark'));
    const animated = markRules.filter(r => r.props.has('animation') && r.props.get('animation') !== 'none');
    expect(animated.map(r => r.selector), 'connecting is meant to be the only animated state').toEqual([
      '.brand-mark[data-conn="connecting"] .brand-mark-glyph',
    ]);

    const cancels = markRules.filter(r =>
      r.atRules.includes('prefers-reduced-motion') && r.props.get('animation') === 'none');
    expect(
      cancels.map(r => r.selector),
      'the reduce override must repeat the animated selector, or it loses on specificity',
    ).toEqual(animated.map(r => r.selector));

    // Without this the connecting mark would sit at full strength, i.e. look
    // exactly like connected, for anyone who asked for no motion.
    expect(cancels[0].props.get('opacity')).toBe('var(--header-mark-connecting-opacity-min)');
  });

  it('renders one variant, so both panes show the same mark', () => {
    for (const dead of ['brand-mark-brand', 'brand-mark-muted']) {
      expect(markCss.includes(dead), `${dead} was replaced by the single plain rendering`).toBe(false);
    }
  });

  it('the threads row reads at the same strength, and outranks the bar to do it', () => {
    // The mobile threads row's mark takes the icon run's BOX but not its muted
    // colour: it is the same brand as the thread pane's connected mark and must
    // read as bright. The `.app-header` prefix is the load-bearing part, since
    // this file is imported before panels/shell.css, whose `.app-header
    // .icon-btn` muted rule would otherwise tie and win on source order.
    const row = cssRules(markCss).find(r => r.selector.includes('.brand-mark-row')
      && r.props.has('color'));
    expect(row?.selector, 'a bare .icon-btn.brand-mark-row loses to the bar')
      .toBe('.app-header .icon-btn.brand-mark-row');
    expect(row?.props.get('color')).toBe('var(--header-fg)');
  });
});

describe('the press cue changes a transform value, never creates one', () => {
  // `transform: none` to `scale(0.93)` is a change of KIND, not of value. An
  // untransformed box is neither a stacking context nor a containing block nor
  // its own compositing layer; a transformed one is all three. A press that
  // builds that structure inside the brand cluster makes Safari re-rasterise
  // the cluster. The mark, both chevrons and the workspace name then shift
  // while the button is held.
  //
  // A source scan because nothing this repo can drive reproduces it. It is not
  // a LAYOUT move (`getBoundingClientRect` is identical held and at rest), and
  // Playwright's WebKit is a different port from Safari and paints it steady.
  //
  // Parsed rather than string-matched: the reduced-motion note further down
  // this stylesheet quotes `.brand-mark .brand-mark-glyph { animation: none }`,
  // which CONTAINS the needle a `block()` lookup would search for.
  const rule = (selector: string) => {
    const found = cssRules(markCss).filter(r => r.selector === selector);
    expect(found.length, `expected exactly one \`${selector}\` rule`).toBe(1);
    return found[0].props;
  };

  it('the glyph is always transformed, so the press interpolates a matrix', () => {
    const glyph = rule('.brand-mark-glyph');
    expect(glyph.get('transform'), 'and it must be the identity, so it changes no geometry')
      .toBe('scale(1)');
    expect(glyph.get('will-change'), 'the layer must exist before the finger lands')
      .toBe('transform');
  });

  it('and the pressed state is a value on the same property', () => {
    // If the cue ever moves to another property (or the resting transform is
    // dropped as "dead code"), the pairing above stops meaning anything.
    expect(rule('.brand-mark:active .brand-mark-glyph').get('transform')).toBe('scale(0.93)');
    expect(rule('.brand-mark-glyph').get('transition'),
      'the transform must still be the animated property')
      .toContain('transform var(--duration-fast)');
  });
});

describe('the badge rides the mark rather than sitting beside it', () => {
  const badge = block(markCss, '.brand-mark-slot .badge.brand-badge {');

  it('is out of flow, so it cannot widen the slot', () => {
    // The load-bearing half. The slot is the centred cluster's middle member,
    // so an in-flow badge moves the mark off the row's axis for as long as an
    // engine build or an update is pending.
    expect(decl(badge, 'position')).toBe('absolute');
    // The flex-slot lift and tuck, which would offset the corner placement.
    expect(decl(badge, 'margin'), 'the superscript margins must be cleared').toBe('0');
  });

  it('takes the GLYPH corner, derived, not the tap target corner', () => {
    // `top/right: 0` is the tap target's corner, and it lands on the glyph only
    // while the target happens to hug it. Both are style-remote tunables, so a
    // wider target floats the badge clear of the mark's ink. Half the
    // difference between the host's box and the glyph IS the glyph's corner at
    // any tuning, so the offset must reference --header-mark-size.
    //
    // Declared on the SLOT, so both badges inherit one arithmetic: the state
    // badge takes the top corner and the unread count takes the bottom.
    // Newline-anchored: `.header-nav-cluster > .brand-mark-slot {` carries the
    // same substring and is the earlier match.
    expect(decl(block(markCss, '\n.brand-mark-slot {'), '--brand-badge-corner'))
      .toBe('calc((100% - var(--header-mark-size)) / 2)');
    for (const side of ['top', 'right']) {
      expect(decl(badge, side), `${side} must derive the corner, not sit at 0`)
        .toBe('calc(var(--brand-badge-corner) + var(--header-mark-badge-offset))');
    }
    // The nudge stays a nudge: on the corner by default.
    expect(decl(block(markCss, ':root {'), '--header-mark-badge-offset')).toBe('0rem');
  });

  it('leaves the mark\'s sparkle alone: the unread count takes the OTHER corner', () => {
    // LucidosMarkIcon puts its sparkle at the TOP-right. The state badge covers
    // it, but only while a build runs or an update waits. An unread count is
    // resident, so parking it there would leave the brand as three plain
    // squares for as long as anything is unread.
    const count = block(markCss, '.brand-mark-slot .badge.brand-unread-badge {');
    expect(decl(count, 'bottom'), 'the count must ride the bottom corner')
      .toBe('calc(var(--brand-badge-corner) + var(--header-mark-badge-offset))');
    // Required, not tidiness: the base `.badge` sets `top`, and a box given both
    // `top` and `bottom` stretches between them into a tall pill.
    expect(decl(count, 'top'), 'the base badge top must be released').toBe('auto');
    expect(decl(badge, 'bottom'), 'the state badge must stay on the top corner').toBeNull();
    // The tap belongs to the mark underneath, which opens the menu. This is a
    // positioned span over a SIBLING button, so its own taps would die.
    expect(decl(count, 'pointer-events')).toBe('none');
  });

  it('stays inside the host box, so no clip can shave it and no measure sees it', () => {
    // `.pane-header-brand-label` clips both axes on the packaged macOS webview,
    // and WorkspaceNameLabel sums the slot's width to decide whether the
    // workspace name still fits. A transform that centred the badge ON the
    // corner would push half of it outside the slot and break both.
    expect(decl(badge, 'transform'), 'the badge must not overflow its slot').toBeNull();
  });

  it('names the artwork inset once, and both compensations read it', () => {
    // LucidosMarkIcon draws inside `translate(13 13) scale(0.74)`. The menu
    // row scales the paint back up by its reciprocal; a literal there is a
    // second copy of a number only the icon knows.
    expect(decl(block(markCss, ':root {'), '--header-mark-art-scale')).toBe('0.74');
    expect(decl(block(markCss, '.brand-menu-version > svg {'), 'transform'))
      .toBe('scale(calc(1 / var(--header-mark-art-scale)))');
    const icons = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf-8');
    expect(icons, 'the mark no longer carries the inset the token restates')
      .toContain('scale(0.74)');
  });

  it('lets a tap through the READY badge, which listens for nothing', () => {
    // Out of flow, the "!" span is a hit target that swallows taps on the
    // mark's corner: what is under it is the mark's BUTTON, a sibling, so there
    // is nothing for the tap to bubble to. The busy badge is a real button and
    // must keep its own events, hence the `:not()` rather than a paired rule.
    const passthrough = block(markCss, '.brand-mark-slot .badge.brand-badge:not(.brand-badge-action) {');
    expect(decl(passthrough, 'pointer-events')).toBe('none');
    const base = block(markCss, '.brand-mark-slot .badge.brand-badge {');
    expect(
      decl(base, 'pointer-events'),
      'the shared rule must not disable events on the badge that IS a button',
    ).toBeNull();
  });

  it('beats both sheets that would re-corner it, on specificity', () => {
    // panels/shell.css resets `top` at (0,2,0) and mobile.css re-corners every
    // `.badge`; this file is imported before both, so a single-class selector
    // here would parse fine, read fine, and lose.
    const targeting = rulesTargeting(markCss, 'brand-badge').map(r => r.selector);
    expect(targeting).toContain('.brand-mark-slot .badge.brand-badge');
  });

  it('reins the busy badge hit area in, so it cannot swallow the mark', () => {
    // A square centred on the badge covers the menu's own tap target: the
    // bottom and left edges have to stay ON the badge.
    const hit = block(markCss, '.brand-mark-slot .brand-badge-action::after {');
    const inset = decl(hit, 'inset') ?? '';
    // Split on whitespace would cut the `calc()`s up, so read the two ends
    // instead: the reach goes on top and right, and the other two edges are 0.
    expect(inset).toContain('--header-mark-badge-hit-reach');
    expect(
      inset.endsWith(' 0 0'),
      `growing down or left is what reaches the mark's centre, got "${inset}"`,
    ).toBe(true);
  });
});

describe('the nav chevrons are pinned to one shared span', () => {
  const cluster = block(markCss, '.header-nav-cluster {');

  it('the cluster is a fixed-width centred box', () => {
    expect(decl(cluster, 'position')).toBe('absolute');
    expect(decl(cluster, 'justify-content')).toBe('space-between');
    // The width is a NAMED quantity rather than an inline clamp, because the
    // threads row's title reserve reads it too (see the reserve test below).
    expect(decl(cluster, 'width')).toBe('var(--header-nav-cluster-width)');
    const span = decl(block(markCss, ':root {'), '--header-nav-cluster-width') ?? '';
    for (const token of ['--header-nav-min-span', '--header-nav-edge-reserve', '--header-nav-span']) {
      expect(span, 'the span must clamp, or it overlaps the edge clusters at high ui-scale').toContain(token);
    }
  });

  it('the cluster SPANS the row vertically, so its own height cannot place it', () => {
    // `top: 50%` plus a `translateY(-50%)` positions the box by half its OWN
    // height, and the three rows fill their clusters differently: the thread
    // row's is as tall as the mark (2.1rem), the content row's as tall as a
    // chevron (1.75rem). Each then rounds its own half. WebKit landed the two
    // 0.14px apart at the 18px mobile root, enough to move the anti-aliasing
    // of a hairline chevron on a phone. Spanning the row removes the term:
    // flexbox centres the members against one identical box.
    expect(decl(cluster, 'top')).toBe('0');
    expect(decl(cluster, 'bottom')).toBe('0');
    expect(decl(cluster, 'height'), 'the inset pair is what spans it; a height re-opens the rounding')
      .toBeNull();
    expect(decl(cluster, 'transform'), 'the horizontal centring only')
      .toBe('translateX(-50%)');
    expect(decl(cluster, 'align-items'), 'flexbox owns the vertical placement now').toBe('center');
  });

  it('the threads pane hangs its mark off the same span, at the trailing edge', () => {
    // The one glyph present on all three mobile rows must not move as the user
    // swipes between them. It lands on the forward chevron's column only if it
    // takes the SAME box and is pinned to its end. The base rule's
    // `space-between` puts a lone member at the START, so this override is
    // what makes it the trailing edge.
    const end = block(markCss, '.header-mark-end-cluster {');
    expect(decl(end, 'justify-content')).toBe('flex-end');
    for (const own of ['width', 'max-width', 'transform', 'left', 'right']) {
      expect(decl(end, own), `a ${own} here would move the mark off the chevron column`).toBeNull();
    }
  });

  it('the threads title reserve is derived from that span, not from a rem constant', () => {
    // The mark's inner edge is half the CLUSTER in from the row's middle, and
    // the cluster is clamped against the row. So a reserve that restates a rem
    // constant agrees with it at one ui-scale only.
    const title = block(mobileCss, '.mobile-header-title {');
    const reserve = decl(title, 'max-width') ?? '';
    expect(reserve, 'the reserve must read the cluster it has to clear')
      .toContain('var(--header-nav-cluster-width)');
    expect(reserve, "and the mark's own button box").toContain('var(--mobile-header-icon-box)');
    expect(
      decl(block(mobileCss, '.mobile-header-row .icon-btn.header-icon {'), 'width'),
      'the buttons and the reserve must read one box size, or they disagree',
    ).toBe('var(--mobile-header-icon-box)');
  });

  it('the content pane does not size its cluster differently', () => {
    // The whole point is that both panes use the same box. A width or a
    // transform here would move the content pane's chevrons off the thread
    // pane's, which is what the fixed span exists to prevent.
    const title = block(markCss, '.header-title-cluster {');
    expect(decl(title, 'width')).toBeNull();
    expect(decl(title, 'max-width')).toBeNull();
    expect(decl(title, 'transform')).toBeNull();
  });

  it('the empty span cannot swallow a tap aimed at the row underneath', () => {
    // The box is a fixed span with its members at the ends, absolutely
    // positioned over the row, so most of it is transparent and raised. A
    // transparent div is a hit target all the same. A tap in the empty part
    // would reach nothing instead of the edge control the user aimed at, so
    // the pair stays together: none on the box, auto back on the members.
    expect(decl(cluster, 'pointer-events')).toBe('none');
    const restore = cssRules(markCss)
      .filter(r => /\.header-nav-cluster > \*$/.test(r.selector))
      .filter(r => r.props.get('pointer-events') === 'auto');
    expect(
      restore.length,
      'the members must get their events back, or nothing in the cluster is tappable',
    ).toBe(1);

    // The restore must stand down while the app is deliberately inert. Both
    // regimes set `none` on an ANCESTOR and let it inherit, so an UNGATED value
    // on the members beats them at any specificity and punches a live hole in
    // each. Gate here rather than naming the cluster in those two rules: that
    // also lets the overlay's `[data-overlay-anchor]` exemption win on the
    // chevron that opened its own menu.
    for (const state of ['data-keyboard-active', 'data-overlay-open']) {
      expect(
        restore[0].selector,
        `the restore must not apply under [${state}], whose inert only inherits`,
      ).toContain(`:not([${state}])`);
    }
  });

  it('the chevrons and the mark cannot be squeezed by a long title', () => {
    const fixed = cssRules(markCss).find(r => r.selector.includes('.header-nav-cluster > .icon-btn'));
    expect(fixed?.props.get('flex')).toBe('0 0 auto');
  });
});

describe('the desktop brand span pays for the same pattern the same way', () => {
  it('none on the box, auto on the members, and the restore stands down behind an overlay', () => {
    // `.pane-header-brand-label` is the desktop copy of the cluster above: a
    // fixed span absolutely centred over the row, mostly transparent, so it
    // takes the same pair for the same reason. It inherits the same trap. A
    // value applied directly to a child beats an inherited one at any
    // specificity. An ungated restore therefore leaves the chevrons and the
    // brand centre live behind the scrim.
    //
    // Only the overlay gate here, where the mobile twin also carries
    // `[data-keyboard-active]`. That regime is a mobile.css rule inside the
    // mobile breakpoint and this one is in the desktop breakpoint, so the two
    // can never co-apply.
    const label = cssRules(shellCss).find(
      r => r.selector === '.app-header .pane-header-brand .pane-header-brand-label',
    );
    expect(label?.props.get('pointer-events'), 'the span must pass clicks through').toBe('none');

    const restore = cssRules(shellCss)
      .filter(r => /\.pane-header-brand-label > \*$/.test(r.selector))
      .filter(r => r.props.get('pointer-events') === 'auto');
    expect(
      restore.length,
      'the members must get their events back, or nothing in the brand is clickable',
    ).toBe(1);
    expect(
      restore[0].selector,
      'the restore must not apply under [data-overlay-open], whose inert only inherits',
    ).toContain(':not([data-overlay-open])');
  });
});

describe('the deleted collapse measurement left nothing behind', () => {
  it('no stylesheet reads a property nothing publishes any more', () => {
    const orphans = ['--mobile-content-title-max', '--mobile-content-title-shift'];
    const sheets: string[] = [];
    const walk = (dir: string): void => {
      for (const entry of readdirSync(dir, { withFileTypes: true }) as Array<{ name: string; isDirectory(): boolean }>) {
        const full = resolve(dir, entry.name);
        if (entry.isDirectory()) walk(full);
        else if (entry.name.endsWith('.css')) sheets.push(full);
      }
    };
    walk(stylesDir);
    for (const sheet of sheets) {
      const css = readFileSync(sheet, 'utf-8');
      for (const orphan of orphans) {
        // A comment may still explain why the property is gone; a `var()` read
        // is what would silently resolve to its fallback forever.
        expect(css.includes(`var(${orphan}`), `${sheet} still reads ${orphan}`).toBe(false);
      }
    }
  });
});
