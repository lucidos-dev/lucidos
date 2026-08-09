/**
 * Source scans over the mobile header's three structural promises. Each is a
 * property of the STYLESHEET rather than of a rendered frame, so each is
 * cheaper and sharper to assert here than in a browser: the e2e specs measure
 * the painted result, this pins the rule that produces it.
 *
 * 1. All three mobile header rows are the same height BY CONSTRUCTION. Under
 *    the `min-height` this replaces, each row was as tall as its own tallest
 *    in-flow control, and the threads row alone carried the Lucidos mark in flow
 *    (the thread pane's sat in the absolutely-positioned nav cluster), so it
 *    stood 0.1rem taller than the other two. Every row's mark is in a cluster
 *    now; the fixed height is what guards the next in-flow control.
 * 2. The mark says its connection state in ONE dimension, strength. Connected
 *    is the brand at full light and nothing else; the other two recede from it.
 *    That holds only if nothing anywhere paints a ring, a tint or a hue onto
 *    any of the three.
 * 3. The badge rides the mark's corner out of flow, so it cannot widen the slot
 *    and slide the centred mark off the row's axis.
 * 4. The chevrons are pinned by one shared span, so the thread pane and the
 *    content pane put them in the same two places, and the threads pane's mark
 *    takes that same span's trailing edge, so all three rows agree on where
 *    that column is.
 *
 * Plus an orphan check on the two custom properties the deleted mobile collapse
 * measurement used to publish. That one is not fussiness: an `env`-style
 * `var(--gone, fallback)` left behind reads as the fallback and a spec that
 * asserted on the published value reads it as `0`, so both halves fail silently
 * rather than loudly.
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

/** The menu carries the Restart control, and the toast most likely to be on
 *  screen when a user reaches for it is the persistent "Restart needed" /
 *  "New version available" one. `--z-modal` (2300) sits deliberately BELOW
 *  `--z-toast` (2400), so the ordinary modal layer would put the toast on top of
 *  the very control the menu was opened for. The retired workspace switcher
 *  rebased above the toast layer for exactly this reason; the menu inherited the
 *  surface and has to inherit the rebase with it.
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
    // The cascade half, which is where this silently failed before: a media
    // query adds no specificity, so an override has to reach the animated
    // rule's own specificity to win at all. Asserting that the reduce block
    // targets the SAME selector is the durable way to say that, since a
    // broader one would parse fine, read fine, and never apply.
    // Scoped to the MARK's own rules (`.brand-mark…`), not to every rule in the
    // file. The stylesheet also dresses the menu, whose panel and scrim each
    // carry the one-shot `modal-in` entrance every overlay in the app uses
    // (global/modal-overlay.css, and the switcher's scrim in panels/shell.css).
    // What this test is about is the mark's CONNECTION states, where a looping
    // animation is the signal and cancelling it has to leave the state legible.
    // A one-shot fade shared with every other overlay is a different question,
    // and sweeping it in here would make this a tripwire on any animation the
    // file ever grows rather than a guard on the cascade.
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
    // while the target happens to hug it. Both are style-remote tunables: a
    // 2.6rem target around a 1.8rem glyph left the badge 8px up and 8px right of
    // the glyph box, floating clear of the mark's ink. Half the difference
    // between the host's own box and the glyph IS the glyph's corner, at any
    // tuning, so the offset must reference --header-mark-size.
    expect(decl(badge, '--brand-badge-corner'))
      .toBe('calc((100% - var(--header-mark-size)) / 2)');
    for (const side of ['top', 'right']) {
      expect(decl(badge, side), `${side} must derive the corner, not sit at 0`)
        .toBe('calc(var(--brand-badge-corner) + var(--header-mark-badge-offset))');
    }
    // The nudge stays a nudge: on the corner by default.
    expect(decl(block(markCss, ':root {'), '--header-mark-badge-offset')).toBe('0rem');
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

  it('the threads pane hangs its mark off the same span, at the trailing edge', () => {
    // The ask: the one glyph present on all three mobile rows must not move as
    // the user swipes between them. It lands on the forward chevron's column
    // only if it takes the SAME box and is pinned to its end. `space-between`
    // (the base rule) puts a lone member at the START, so the override is what
    // makes this the trailing edge rather than the leading one.
    const end = block(markCss, '.header-mark-end-cluster {');
    expect(decl(end, 'justify-content')).toBe('flex-end');
    for (const own of ['width', 'max-width', 'transform', 'left', 'right']) {
      expect(decl(end, own), `a ${own} here would move the mark off the chevron column`).toBeNull();
    }
  });

  it('the threads title reserve is derived from that span, not from a rem constant', () => {
    // The mark's inner edge is now half the CLUSTER in from the row's middle,
    // and the cluster is clamped against the row, so the two only agree at one
    // ui-scale if the reserve restates a constant. They kissed at 150% under
    // the old `calc(100% - 10.5rem)`.
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
    // positioned over the row, so most of it is transparent and raised: a tap
    // in the empty part reaches nothing instead of the edge control the user
    // aimed at. `.mobile-dot-indicator` learned this exact rule ("a transparent
    // div is a hit target all the same"), so the pair has to stay together:
    // none on the box, auto back on the members.
    expect(decl(cluster, 'pointer-events')).toBe('none');
    const restore = cssRules(markCss)
      .filter(r => /\.header-nav-cluster > \*$/.test(r.selector))
      .filter(r => r.props.get('pointer-events') === 'auto');
    expect(
      restore.length,
      'the members must get their events back, or nothing in the cluster is tappable',
    ).toBe(1);

    // And the restore must stand down while the app is deliberately inert. Both
    // regimes set `none` on an ANCESTOR and let it inherit, so an UNGATED value
    // on the members beats them at any specificity and quietly punches a live
    // hole in each: the chevrons would answer a stray thumb with the keyboard up
    // (`:root[data-keyboard-active] .app-header`, mobile.css) and stay live
    // behind an open overlay (`:root[data-overlay-open] .app-shell > *`,
    // global/modal-overlay.css). Gating here rather than naming the cluster in
    // those two rules is also what leaves the overlay's `[data-overlay-anchor]`
    // exemption able to win on the chevron that opened its own history menu.
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
