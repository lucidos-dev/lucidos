/**
 * Source scans over the desktop header's vertical centering on the packaged
 * macOS build.
 *
 * The reclaimed title-bar band (`.titlebar-strip`, sized to `--titlebar-inset`)
 * paints the same blue directly above `.app-header`, so the user sees ONE bar.
 * A region centered on the header alone reads as sitting low in it. The
 * correction is one shared term, `--header-band-lift`, subtracted from each
 * region's centering transform.
 *
 * Scans rather than a browser spec because none of this reproduces in a
 * rendered frame off the macOS Tauri build: `--titlebar-inset` is unset
 * everywhere else, so every term below collapses to `0px` and each failure
 * paints identically to the fix.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, cssRules, decl, rulesTargeting, selectorList, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');

const shellCss = styles('panels/shell.css');
const drawerCss = styles('drawer.css');
const shellRules = cssRules(shellCss);

const DESKTOP = '@media (min-width: 769px)';

/** The one desktop rule with this exact selector. Exactly one, because
 *  `.threads-header` also has a base rule outside the media query and a first
 *  textual match would read the wrong one. */
function desktopRule(selector: string): CssRule {
  const found = shellRules.filter(r => r.selector === selector && r.atRules === DESKTOP);
  expect(found.length, `expected exactly one desktop \`${selector}\` rule`).toBe(1);
  return found[0];
}

const CENTER_ON_BAND = 'translateY(calc(-50% - var(--header-band-lift)))';

describe('the desktop header centers on the whole bar, not on the header alone', () => {
  it('the lift is half the reclaimed title-bar band, so it is 0px off the macOS Tauri build', () => {
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    expect(root?.props.get('--header-band-lift')).toBe('calc(var(--titlebar-inset, 0px) / 2)');
  });

  it.each([
    ['.threads-header'],
    ['.app-header .pane-header-brand'],
    ['.app-header .content-header-elements'],
    // The drawer toggle, in the header's LEFT-MOST slot. It TRAVELS
    // horizontally between the two drawer states
    // (header-drawer-toggle-travel.test.ts), so the lift is the only vertical
    // term it may carry: a second one would make the icon rise or dip as it
    // slid.
    ['.thread-toggle-slot'],
  ])('%s takes the lift', selector => {
    expect(desktopRule(selector).props.get('transform')).toBe(CENTER_ON_BAND);
  });

  it('the Filter button rides its row, with no vertical term of its own', () => {
    // The drawer-open half of the same slot, and a plain flex child of the
    // lifted `.threads-header`. Any vertical term here moves it off the line
    // the rest of the bar sits on.
    const slotRules = shellRules.filter(
      r => r.selector.includes('.view-selector-slot') && r.atRules === DESKTOP,
    );
    for (const rule of slotRules) {
      for (const prop of ['top', 'bottom', 'transform', 'margin-top', 'position']) {
        expect(rule.props.get(prop), `${rule.selector} { ${prop} }`).toBeUndefined();
      }
    }

    // The base rule is the only positioned ancestor left for the
    // needs-attention badge. Drop it and the count silently moves onto the
    // drawer's corner.
    const base = shellRules.find(r => r.selector === '.view-selector-slot' && r.atRules === '');
    expect(base?.props.get('position'), 'the badge hangs off this').toBe('relative');
  });
});

describe('the brand region is as tall as the label it centres', () => {
  // `.pane-header-brand` declares no height, so it shrinks to its one IN-FLOW
  // child, the actions cluster. What it has to hold is the brand label, which
  // is out of flow and as tall as the taller of --header-icon-box and
  // --header-mark-tap. Past the icon box the label hangs out of the box.
  //
  // The region's clip behaves as a scroll container on the packaged macOS
  // webview. Vertical overflow is scrolled away when a control inside is
  // clicked, then restored: the mark and both chevrons jump. No WebDriver
  // reaches WKWebView (ADR 0016), so only the stylesheet can pin this.
  it('the region reserves the label height, so a retuned mark cannot overflow it', () => {
    const brand = desktopRule('.app-header .pane-header-brand');
    expect(brand.props.get('min-height'))
      .toBe('max(var(--header-icon-box), var(--header-mark-tap))');
    // `min-height`, never `height`: a taller in-flow cluster must grow the
    // region rather than overflow it the other way.
    expect(brand.props.get('height'), 'a fixed height would re-open the overflow')
      .toBeUndefined();
  });

  it('both terms are the real box heights, not numbers that once matched them', () => {
    // CSS cannot read a rule's own declaration back out. Each term above
    // points at a token another stylesheet owns. If `.brand-mark` stops being
    // sized by --header-mark-tap, the reserve covers the wrong box.
    const markRules = cssRules(readFileSync(resolve(stylesDir, 'header-mark.css'), 'utf-8'));
    const mark = markRules.find(r => r.selector === '.brand-mark');
    expect(mark?.props.get('height'), '.brand-mark drifted from --header-mark-tap')
      .toBe('var(--header-mark-tap)');
  });
});

describe('on the overlay build the leading control only steps sideways', () => {
  it('the lights reserve is derived from the x the shell places them at', () => {
    // --titlebar-lights-x is stamped pre-paint by `titlebar_inset_script` from
    // the same constant `src/traffic_lights.rs` places the cluster with. So
    // the room the row keeps clear is arithmetic on a placement we made, and a
    // literal here would be free to drift from it.
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    expect(root?.props.get('--titlebar-lights-reserve')).toBe(
      'calc(var(--titlebar-lights-x, 10px) + var(--titlebar-lights-cluster)'
      + ' + var(--titlebar-lights-gap))',
    );
  });

  it('every term of the reserve is px, because the lights do not scale with the UI', () => {
    // rem in any of the three is a real bug: the OS cluster is fixed px, so at
    // UI_SCALE_MIN a rem reserve shrinks inside it and the control lands on
    // the lights.
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    expect(/var\(--titlebar-lights-x, (\d+px)\)/.exec(reserve)?.[1]).toMatch(/^\d+px$/);
    for (const term of ['--titlebar-lights-cluster', '--titlebar-lights-gap']) {
      expect(root?.props.get(term), term).toMatch(/^\d+px$/);
    }
  });

  it('the derived reserve still comes out at the 80px every reader was sized for', () => {
    // Five call sites are laid out against this number: three leading
    // controls, the centred threads title's clamp, and paneMinimums.ts's
    // drawer floor. So the sum is pinned, not just the shape. 10 (our x) + 60
    // (the cluster: three 14pt button frames at a 23pt pitch, measured) + 10
    // (what is left over).
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    const terms = [...reserve.matchAll(/(\d+)px/g)].map(m => Number(m[1]));
    // The first px literal in the reserve is the x's fallback.
    const [x] = terms;
    const cluster = parseInt(root!.props.get('--titlebar-lights-cluster')!, 10);
    const gap = parseInt(root!.props.get('--titlebar-lights-gap')!, 10);
    expect(x + cluster + gap).toBe(80);
  });

  it('splits that slack evenly, so the lights are centred in the room they get', () => {
    // The one thing the sum above cannot see. Equal terms put 11px of air on
    // each side of the drawn cluster, the 12pt circle sitting 1pt inside its
    // 14pt frame. The reserve, and every reader of it, is untouched. Put the
    // whole slack in front and the row reads as cramped against the lights.
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    const x = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    expect(parseInt(root!.props.get('--titlebar-lights-gap')!, 10)).toBe(x);
  });

  it('the reserve is registered, so its JS reader still gets a length', () => {
    // `store/paneMinimums.ts` parseFloats this property off the root. An
    // UNREGISTERED custom property computes to its token sequence with the
    // calc UNEVALUATED. So a reserve that is not a literal reads as NaN, and
    // falls back silently and forever. Registering it computes the length.
    const body = block(shellCss, '@property --titlebar-lights-reserve');
    expect(decl(body, 'syntax')).toBe("'<length>'");
    // The initial value covers a viewport below the desktop breakpoint, where
    // the block that declares the reserve does not run. It must be the same sum
    // the arithmetic produces, or the drawer's floor would differ across the
    // breakpoint for no reason.
    expect(decl(body, 'initial-value')).toBe('80px');
    // And it must sit OUTSIDE the media query, which is not a style choice:
    // `@property` is not a conditional rule and is dropped if nested in one.
    const at = shellCss.indexOf('@property --titlebar-lights-reserve');
    expect(at, 'the registration is missing').toBeGreaterThanOrEqual(0);
    expect(at, 'the registration must precede the desktop media query')
      .toBeLessThan(shellCss.indexOf(`${DESKTOP} {`));
  });

  it('the fallback x is the x the shell actually places the lights at', () => {
    // CSS cannot read Rust, so the fallback inside the var() is a copy of
    // LIGHTS_X_PX and free to rot. It is what an unstamped document resolves
    // to, and a stale one reserves room for a cluster that is somewhere else.
    const rust = readFileSync(resolve(stylesDir, '../traffic_lights.rs'), 'utf-8');
    const placed = Number(/LIGHTS_X_PX:\s*f64\s*=\s*([\d.]+)/.exec(rust)?.[1]);
    expect(placed, 'LIGHTS_X_PX not found in traffic_lights.rs').toBeGreaterThan(0);

    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    const fallback = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    expect(fallback).toBe(placed);
  });

  it('the reserve still clears a control that is centred on the bar', () => {
    // The sideways step is the only thing keeping the leading control off the
    // lights. It has to clear them for a box that reaches up into the band.
    // --header-icon-box is a copy of a value in another stylesheet, so this
    // also catches it drifting from the button.
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    // Matched as a member of the rule's selector LIST, not as the whole
    // selector text: the box is shared with `.icon-btn.row-icon`, so an exact
    // compare reads `undefined` the moment the two are declared together and
    // the guard silently stops guarding.
    const button = cssRules(styles('global/host-components.css'))
      .filter(r => selectorList(r.selector).includes('.icon-btn.header-icon'));
    const iconBox = button.find(r => r.props.has('height'));
    expect(iconBox, 'no .icon-btn.header-icon rule declaring a height').toBeDefined();
    // Resolve one level of indirection. The button takes its height from a var
    // shared with the two other icon bands. A raw compare would weigh a `var()`
    // against a length and never agree. Resolved from the button's OWN rules,
    // so a narrower rule redeclaring the var for this band wins over whichever
    // copy comes first.
    const height = iconBox!.props.get('height')!;
    const named = /^var\((--[\w-]+)\)$/.exec(height)?.[1];
    const resolved = named ? button.map(r => r.props.get(named)).find(v => v !== undefined) : height;
    expect(resolved, `${named} is declared on no .icon-btn.header-icon rule`).toBeDefined();
    expect(root?.props.get('--header-icon-box'), '--header-icon-box drifted from the button')
      .toBe(resolved);
  });

  it('the drawer toggle clears the lights, and moves in no other direction', () => {
    // It moves the RESTING PLACE, not `left`: only the drawer-shut position
    // differs between the builds. Overriding `left` here would restate the
    // drawer-open rule, and be free to drift from it.
    //
    // The resting place is --brand-lead-inset on :root, and it has to be there
    // rather than on the slot: --brand-side-reserve reads the same value, so
    // the centred brand cluster knows where this control actually is. A private
    // copy on the slot is how the two came apart, which is the overlap the two
    // cases below now rule out.
    expect(desktopRule(':root[data-titlebar-overlay]').props.get('--brand-lead-inset'))
      .toBe('var(--titlebar-lights-reserve)');
    // No rule of its own for the slot on this build: the override is the value.
    expect(shellRules.filter(
      r => r.selector === ':root[data-titlebar-overlay] .thread-toggle-slot',
    ), 'the resting place has two declarations again').toEqual([]);
    // And the base rule is what consumes it, so the override cannot be inert.
    expect(desktopRule('.thread-toggle-slot').props.get('left'))
      .toBe('var(--brand-lead-inset)');
    expect(shellCss, '--thread-toggle-home is retired; one name for one place')
      .not.toContain('--thread-toggle-home');
  });

  it('the reserve the centred cluster clears is the WIDER of the row\'s two ends', () => {
    // The reported overlap, as arithmetic. This reserve was once the trailing
    // end alone, on the reading that the drawer toggle made the leading end the
    // narrower one. That holds on the web build from 100% ui-scale up. It is
    // false on the packaged one at every scale, where the toggle starts 80px
    // in. Below 100% it is false on both: the trailing term is rem and the
    // lights reserve is px.
    const reserve = desktopRule(':root').props.get('--brand-side-reserve')!;
    expect(reserve.startsWith('max('), `--brand-side-reserve is not a max(): ${reserve}`)
      .toBe(true);
    // The leading term names the inset the toggle is actually placed with,
    // rather than restating 0.5rem or 80px.
    expect(reserve).toContain('calc(var(--brand-lead-inset) + var(--header-icon-box))');
    // …and the trailing one is still the actions at their widest.
    expect(reserve).toContain('3 * var(--header-icon-box)');
  });

  it('the cluster\'s floor IS its natural width, so the chevrons hug the mark', () => {
    // Two chevrons and the mark's tap target, touching. Derived rather than
    // picked. A picked 8rem against a natural 6.6rem leaves 1.4rem of slack.
    // That slack is what pushed the back chevron onto the toggle at a narrow
    // Conversation pane. Both centred desktop clusters read this token, so a
    // literal here silently re-widens the Canvas row's floor too.
    expect(desktopRule(':root').props.get('--desktop-nav-min-span'))
      .toBe('calc(2 * var(--header-icon-box) + var(--header-mark-tap))');
  });

  it('the content row keeps the reserve as a FLOOR under the divider', () => {
    // The third control that can hold the header's left-most slot: the
    // hamburger leading the Canvas pane's row. Maximizing that pane takes
    // --split-ratio to 0, so --divider-x is 0 and the base rule lands the
    // hamburger at the window's own left edge, on the lights.
    //
    // A floor rather than a gate on [data-thread-collapsed]: --split-ratio is
    // a persisted RATIO that nothing re-clamps when the window shrinks, so a
    // divider can land under the reserve with no pane collapsed.
    const base = desktopRule('.app-header .content-header-elements').props.get('left');
    expect(base).toBe('calc(var(--divider-x) + var(--divider-width))');

    const rule = desktopRule(':root[data-titlebar-overlay] .app-header .content-header-elements');
    // Two copies of the divider term, which CSS cannot hand back as a resolved
    // length, so this is the drift check that makes the copy safe. Horizontal
    // only: the row keeps the base rule's bar centring, so the hamburger stays
    // on the bar's one line.
    expect(rule.props.get('left')).toBe(`max(${base}, var(--titlebar-lights-reserve))`);
    expect([...rule.props.keys()], 'horizontal-only here too').toEqual(['left']);
  });

  it('the drawer-open row starts after the lights, carrying the button with it', () => {
    // Moving the ROW rather than the button keeps the two drawer states on one
    // height for free: the button stays an ordinary flex child and inherits
    // the row's bar centring.
    const rule = desktopRule(':root[data-titlebar-overlay] .threads-header');
    expect(rule.props.get('padding-left')).toBe('var(--titlebar-lights-reserve)');
  });

  it('the reserve survives the search field taking over the row', () => {
    // The ROW is centred on the bar, so whatever leads it reaches into the
    // band. Hand the reserve back under `.search-active` and the lights sit
    // over the search field's magnifier, eating any click that lands there.
    const searchRules = shellRules.filter(
      r => r.selector.includes('.threads-header') && r.selector.includes('.search-active')
        && r.selector.includes('[data-titlebar-overlay]'),
    );
    for (const rule of searchRules) {
      for (const prop of ['padding', 'padding-left']) {
        expect(rule.props.get(prop), `${rule.selector} { ${prop} }`).toBeUndefined();
      }
    }
  });

  it('every rule that moves a control is gated on the attribute, never on the var', () => {
    // The reserve resolves to 80px everywhere: it has no `--titlebar-inset`
    // term to zero it out, and its stamped input carries a fallback. So a rule
    // keyed on the var alone would indent a web header by the width of traffic
    // lights it does not have.
    const movers = shellRules.filter(
      r => r.atRules === DESKTOP && r.props.get('--titlebar-lights-reserve') === undefined
        && [...r.props.values()].some(v => v.includes('--titlebar-lights-reserve')),
    );
    expect(
      movers.length,
      'expected the three leading-control rules, plus the centred title\'s clamp',
    ).toBe(4);
    for (const rule of movers) {
      expect(rule.selector, rule.selector).toContain('[data-titlebar-overlay]');
    }
  });
});

describe('the Threads title is centred on the pane, not between the icons', () => {
  it('is out of the flex row and pinned to the pane\'s own middle', () => {
    // `flex: 1` centres the title on the GAP between the Filter and Search
    // buttons. Off this build the gap's middle and the pane's coincide. Here
    // the row starts after the lights reserve, which puts the title
    // (reserve - 0.5rem) / 2 right of the pane's middle.
    const title = desktopRule('.threads-header-title');
    expect(title.props.get('position')).toBe('absolute');
    expect(title.props.get('left')).toBe('50%');
    expect(title.props.get('transform')).toBe('translate(-50%, -50%)');
    expect(title.props.get('flex'), 'flex:1 is what centred it on the gap').toBeUndefined();
    // Explicit rather than left to the flex static position, which is a weaker
    // guarantee for an out-of-flow child.
    expect(title.props.get('top')).toBe('50%');
  });

  it('leaves the Search button pinned to the row\'s trailing edge', () => {
    // With the title out of flow the row has two in-flow children left, and
    // this is what holds them apart.
    expect(desktopRule('.threads-header').props.get('justify-content')).toBe('space-between');
  });

  it('clears the WIDER of the row\'s two ends on BOTH sides, per build', () => {
    // A box centred on the pane overlaps a button as soon as it is wider than
    // twice the distance from the pane's middle to that button's inner edge.
    // The clamp is therefore the structural non-overlap guarantee.
    expect(desktopRule('.threads-header-title').props.get('max-width'))
      .toBe('calc(100% - 2 * (var(--threads-title-lead) + var(--header-icon-box) + var(--pane-header-gap)))');
    // The lead is the ONLY thing the overlay build touches, so the doubling
    // lives in one shared clamp and cannot drift between the builds.
    expect([...desktopRule(':root[data-titlebar-overlay] .threads-header-title').props.keys()])
      .toEqual(['--threads-title-lead']);
  });

  it('takes its lead from the row\'s own padding, on each build', () => {
    // The clamp is symmetric about the pane's middle only if the lead it
    // counts twice is what the row keeps clear at its leading end. CSS cannot
    // hand that back as a resolved length, so this is the drift check on the
    // web build's two literals.
    const rowPadding = desktopRule('.threads-header').props.get('padding');
    expect(rowPadding).toBe('0 0.5rem');
    expect(desktopRule('.threads-header-title').props.get('--threads-title-lead'))
      .toBe(rowPadding!.split(' ')[1]);

    const overlayLead = desktopRule(':root[data-titlebar-overlay] .threads-header-title')
      .props.get('--threads-title-lead');
    expect(overlayLead)
      .toBe(desktopRule(':root[data-titlebar-overlay] .threads-header').props.get('padding-left'));
  });

  it('declares the lead on the title, where --pane-header-gap actually resolves', () => {
    // A custom property is substituted on the element that DECLARES it. A
    // `:root` copy would resolve --pane-header-gap against a rule that has
    // never heard of it. That makes the clamp invalid at computed-value time,
    // silently dropping the max-width.
    for (const rule of shellRules) {
      if (rule.props.get('--threads-title-lead') === undefined) continue;
      expect(rule.selector, `${rule.selector} { --threads-title-lead }`)
        .toContain('.threads-header-title');
    }
  });
});

describe('the threads header clips sideways only, on both builds', () => {
  it('every edge is flush, now that nothing leaves the row vertically', () => {
    expect(desktopRule('.threads-header').props.get('clip-path')).toBe('inset(0)');
  });

  it('the overlay build adds no clip of its own', () => {
    expect(desktopRule(':root[data-titlebar-overlay] .threads-header').props.get('clip-path'))
      .toBeUndefined();
  });

  it('carries no clip release for the popout that no longer exists', () => {
    // The filter is a panel in the drawer pane (ThreadFilterPanel). Nothing
    // pops out of this header, so a `:has()` clip release would be a rule
    // nobody can reach.
    expect(shellCss).not.toContain('thread-filter-dropdown');
  });

  it('nothing puts an overflow clip back on it', () => {
    // The row must stay a `clip-path`. `overflow-x: clip` is not available to
    // it: the packaged macOS webview clips BOTH axes as soon as one of them is
    // `clip`, and that is the build this file is about.
    for (const css of [shellCss, drawerCss]) {
      for (const rule of rulesTargeting(css, 'threads-header')) {
        for (const prop of ['overflow', 'overflow-y']) {
          const value = rule.props.get(prop);
          expect(value ?? 'visible', `${rule.selector} { ${prop} }`).toBe('visible');
        }
      }
    }
  });
});

describe('both desktop builds show one bar of the same height', () => {
  it('web and PWA get the whole bar from the header, having no band', () => {
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    expect(root?.props.get('--desktop-bar-height')).toBe('3rem');
    expect(root?.props.get('--app-header-height')).toBe('var(--desktop-bar-height)');
  });

  it('the bar still clears the fixed band at the smallest UI scale', () => {
    // The one term here that does not scale with the bar. --titlebar-inset is
    // 28 fixed px from the OS (`titlebar_inset_script`, lucidos-app/src/lib.rs)
    // while --desktop-bar-height is rem, so shrinking the bar walks the
    // overlay build's header toward zero and then negative. The scale bounds
    // live in the shared appearance contract, which the two FOUC scripts read
    // too.
    const appearance = readFileSync(
      resolve(stylesDir, '../../../../packages/lucidos-sdk/src/appearance.ts'), 'utf-8',
    );
    const minScale = Number(/UI_SCALE_MIN\s*=\s*([\d.]+)/.exec(appearance)?.[1]);
    expect(minScale, 'UI_SCALE_MIN not found in appearance.ts').toBeGreaterThan(0);

    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const barRem = Number(/^([\d.]+)rem$/.exec(root!.props.get('--desktop-bar-height')!)?.[1]);
    expect(barRem, '--desktop-bar-height is no longer a plain rem').toBeGreaterThan(0);

    const TITLEBAR_INSET_PX = 28;
    const barAtMinScale = barRem * 16 * (minScale / 100);
    expect(
      barAtMinScale,
      `a ${barRem}rem bar is ${barAtMinScale}px at ${minScale}%, under the ${TITLEBAR_INSET_PX}px band`,
    ).toBeGreaterThan(TITLEBAR_INSET_PX);
  });

  it('the overlay build SUBTRACTS its band, so the sum holds at every UI scale', () => {
    // Two literals that add up at 100% do not add up anywhere else:
    // --titlebar-inset is fixed px and --desktop-bar-height is rem.
    // Subtraction makes the equality structural.
    expect(desktopRule(':root[data-titlebar-overlay]').props.get('--app-header-height'))
      .toBe('calc(var(--desktop-bar-height) - var(--titlebar-inset, 0px))');
  });

  it('no mobile viewport is touched', () => {
    // The base token in global/base.css carries the notch's
    // env(safe-area-inset-top); every override here drops it, so one escaping
    // the desktop media query would cut the inset off an iPhone header.
    for (const rule of shellRules) {
      if (rule.props.get('--app-header-height') === undefined) continue;
      expect(rule.atRules, `${rule.selector} { --app-header-height }`).toBe(DESKTOP);
    }
  });
});
