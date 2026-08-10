/**
 * Source scans over the desktop header's vertical centering on the packaged
 * macOS build.
 *
 * The reclaimed title-bar band (`.titlebar-strip`, sized to `--titlebar-inset`)
 * paints the same blue directly above `.app-header`, so what the user sees is
 * ONE bar. A header region centered on the header alone therefore reads as
 * sitting low in it, and the correction is a single shared term,
 * `--header-band-lift`, subtracted from each region's centering transform.
 *
 * A source scan rather than a browser spec because NONE of this reproduces in a
 * rendered frame off the macOS Tauri build: `--titlebar-inset` is unset
 * everywhere else, so every term below collapses to `0px` and each failure
 * paints identically to the fix. Five things are pinned:
 *
 * 1. EVERY region takes the lift, the two controls that can occupy the header's
 *    LEFT-MOST slot included. They were once the exception, raised clear out of
 *    the header and pinned to the window's top edge beside the traffic lights,
 *    and on the packaged build that read as one icon sitting higher than its
 *    neighbours. The user rejected it.
 * 2. That slot is still where the lights float, so on the overlay build each of
 *    the three controls that can hold it steps SIDEWAYS to clear them, and that
 *    step is now the only thing the build does to any of them. It has to be,
 *    rather than a vertical dodge: a 2.25rem box centred on the bar reaches up
 *    into the band whatever else is true, and the lights are centred on that
 *    same bar now (we place them, `src/traffic_lights.rs`), so a leading control
 *    at the row's own padding would paint over one. Every rule stays gated on
 *    `[data-titlebar-overlay]`, never on `--titlebar-inset` being `0px`: the
 *    reserve resolves to 80px everywhere and would indent a web header by the
 *    width of lights it does not have. The reserve is DERIVED from the x the
 *    shell places the cluster at rather than stated as a literal, which is the
 *    other half of what these scans pin.
 * 3. `.threads-header` clips SIDEWAYS ONLY, and with the leading control back on
 *    the row every edge is flush, so the clip is `inset(0)` on both builds. It
 *    must stay a `clip-path` anyway: `overflow-x: clip` is not available,
 *    because the packaged macOS webview clips both axes as soon as one of them
 *    is `clip`, which is the very build this file is about.
 * 4. Both desktop builds resolve to the same bar, and no override escapes the
 *    desktop media query into a mobile viewport.
 * 5. The brand region reserves the height of the label it centres, rather than
 *    taking whatever height its trailing actions cluster happens to have. Same
 *    build again, and the same "no rendered frame can see it" problem: the
 *    shipped mark fits the region exactly, and only a retuned --header-mark-tap
 *    makes the label hang out of a box whose clip that webview implements as a
 *    scroll container.
 * 6. The Threads title is centred on the PANE rather than on the gap between
 *    the two buttons flanking it, which is the same distinction and the same
 *    build: the sideways step in (2) is exactly what pushed the gap's middle
 *    off the pane's. This one DOES paint differently off the overlay build, but
 *    only in the failing direction, so it is scanned beside its cause rather
 *    than in a browser spec that could never see the build that reported it.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, cssRules, decl, rulesTargeting, type CssRule } from './css-rule-helpers';

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

/** The centering transform of a region that sits on the whole bar. */
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
    // The drawer toggle, in the header's LEFT-MOST slot. It sat the lift out
    // while it was raised into the band instead; taking it is what puts it back
    // on the same line as the rest of the bar's controls. It TRAVELS
    // horizontally between the two drawer states
    // (header-drawer-toggle-travel.test.ts), so the lift is also the only
    // vertical term it may ever carry: a second one would make the icon rise or
    // dip as it slid.
    ['.thread-toggle-slot'],
  ])('%s takes the lift', selector => {
    expect(desktopRule(selector).props.get('transform')).toBe(CENTER_ON_BAND);
  });

  it('the Filter button rides its row, with no vertical term of its own', () => {
    // The drawer-open half of the same slot. It is a plain flex child of
    // `.threads-header`, which is lifted above, so anything here would move it
    // off the line every other control in the bar sits on. There is deliberately
    // no `.threads-header .view-selector-slot` desktop rule at all: the base one
    // (outside the media query) carries only the badge's `position: relative`.
    const slotRules = shellRules.filter(
      r => r.selector.includes('.view-selector-slot') && r.atRules === DESKTOP,
    );
    for (const rule of slotRules) {
      for (const prop of ['top', 'bottom', 'transform', 'margin-top', 'position']) {
        expect(rule.props.get(prop), `${rule.selector} { ${prop} }`).toBeUndefined();
      }
    }

    // …and the base rule is the LAST thing holding the needs-attention badge on
    // the button's corner. Two declarations used to guarantee a positioned
    // ancestor for it (this one, and the `position: absolute` the deleted
    // overlay rule set); with one left, dropping it would silently move the
    // count onto the drawer's corner.
    const base = shellRules.find(r => r.selector === '.view-selector-slot' && r.atRules === '');
    expect(base?.props.get('position'), 'the badge hangs off this').toBe('relative');
  });
});

describe('the brand region is as tall as the label it centres', () => {
  // Same rule as above, the other vertical fact about it. `.pane-header-brand`
  // declares no height, so it shrinks to its one IN-FLOW child, the actions
  // cluster (one --header-icon-box). What it has to hold is the brand label,
  // which is OUT of flow and centred on it, and which is as tall as the taller
  // of the chevrons (--header-icon-box) and the mark's tap target
  // (--header-mark-tap). Those two are 2.25rem and 2.1rem as shipped, so the
  // label fits exactly and the region looks correctly sized; --header-mark-tap
  // is a style-remote tunable, and past the icon box the label hangs out of the
  // box top and bottom.
  //
  // The overflow is what the user sees, by a route that is invisible here: the
  // region's `overflow-x: clip` sits beside an `overflow-y: visible` that the
  // packaged macOS webview does not honour, and its clip behaves as a scroll
  // container, so a few pixels of vertical overflow are scrolled away when a
  // control inside is clicked and restored after. That is the mark and both
  // chevrons jumping up and coming back, reported twice against that build. No
  // WebDriver reaches WKWebView (ADR 0016) and the shipped tuning overflows by
  // zero, so a rendered frame cannot pin this on any surface the repo has. The
  // stylesheet can.
  it('the region reserves the label height, so a retuned mark cannot overflow it', () => {
    const brand = desktopRule('.app-header .pane-header-brand');
    expect(brand.props.get('min-height'))
      .toBe('max(var(--header-icon-box), var(--header-mark-tap))');
    // `min-height`, never `height`: a taller in-flow cluster must still grow the
    // region rather than overflow it in the other direction.
    expect(brand.props.get('height'), 'a fixed height would re-open the overflow')
      .toBeUndefined();
  });

  it('both terms are the real box heights, not numbers that once matched them', () => {
    // CSS cannot read a rule's own declaration back out, so each term above is a
    // reference to a token that some OTHER stylesheet has to keep pointed at the
    // control it names. --header-icon-box is checked against the button further
    // down this file; this is the mark's half. If `.brand-mark` ever stops being
    // sized by --header-mark-tap, the reserve silently covers the wrong box.
    const markRules = cssRules(readFileSync(resolve(stylesDir, 'header-mark.css'), 'utf-8'));
    const mark = markRules.find(r => r.selector === '.brand-mark');
    expect(mark?.props.get('height'), '.brand-mark drifted from --header-mark-tap')
      .toBe('var(--header-mark-tap)');
  });
});

describe('on the overlay build the leading control only steps sideways', () => {
  it('the lights reserve is derived from the x the shell places them at', () => {
    // The whole point of the arrangement: --titlebar-lights-x is stamped
    // pre-paint by `titlebar_inset_script` from the SAME constant
    // src/traffic_lights.rs places the real cluster with, so the room the row
    // keeps clear is arithmetic on a placement we made rather than an estimate
    // of where the OS left buttons we did not control. A literal here would be
    // free to drift from the placement the moment either moved.
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    expect(root?.props.get('--titlebar-lights-reserve')).toBe(
      'calc(var(--titlebar-lights-x, 10px) + var(--titlebar-lights-cluster)'
      + ' + var(--titlebar-lights-gap))',
    );
  });

  it('every term of the reserve is px, because the lights do not scale with the UI', () => {
    // rem in ANY of the three is a real bug: at UI_SCALE_MIN (75%) a 5rem
    // reserve computes to 60px, inside the 60px the OS cluster occupies whatever
    // our root font size is, and the control lands on the lights. The x is
    // checked at its fallback here and at its source below.
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    expect(/var\(--titlebar-lights-x, (\d+px)\)/.exec(reserve)?.[1]).toMatch(/^\d+px$/);
    for (const term of ['--titlebar-lights-cluster', '--titlebar-lights-gap']) {
      expect(root?.props.get(term), term).toMatch(/^\d+px$/);
    }
  });

  it('the derived reserve still comes out at the 80px every reader was sized for', () => {
    // Nothing may move horizontally: five call sites are laid out against this
    // number (three leading controls, the centred threads title's clamp, and
    // store/paneMinimums.ts's drawer floor), and neither the change that derived
    // it nor the one that re-split it was about spending more room. So the sum
    // is pinned, not just the shape. 10 (our x) + 60 (the cluster: three 14pt
    // button frames at a 23pt pitch, measured) + 10 (what is left over).
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    const terms = [...reserve.matchAll(/(\d+)px/g)].map(m => Number(m[1]));
    // The x's fallback plus the two named terms, resolved from this same rule.
    const [x] = terms;
    const cluster = parseInt(root!.props.get('--titlebar-lights-cluster')!, 10);
    const gap = parseInt(root!.props.get('--titlebar-lights-gap')!, 10);
    expect(x + cluster + gap).toBe(80);
  });

  it('splits that slack evenly, so the lights are centred in the room they get', () => {
    // The user's report, and the one thing the sum above cannot see: with the
    // whole 20px in front of the cluster the leading control's box began
    // exactly where the zoom button's ended, and the row read as cramped
    // against the lights. Equal terms put 11px of air on each side of the drawn
    // cluster (the 12pt circle sits 1pt inside its 14pt frame) while leaving the
    // reserve, and therefore every reader of it, untouched. Split it unevenly
    // again and the sum still passes.
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    const x = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    expect(parseInt(root!.props.get('--titlebar-lights-gap')!, 10)).toBe(x);
  });

  it('the reserve is registered, so its JS reader still gets a length', () => {
    // The fifth reader is `store/paneMinimums.ts`, which parseFloats the
    // property off the root to keep the thread drawer's floor from restating a
    // number the CSS owns. An UNREGISTERED custom property computes to its
    // token sequence with vars substituted and the calc UNEVALUATED, so the
    // moment the reserve stopped being a literal that read started answering
    // `calc(20px + 60px + 0px)`, i.e. NaN, i.e. the fallback, silently and
    // forever. Registering restores it. Verified in both shipped engines.
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
    // CSS cannot read Rust, so the fallback inside the var() is a COPY of
    // LIGHTS_X_PX and free to rot: it is what a document that somehow was not
    // stamped resolves to, and a stale one would reserve room for a cluster
    // that is somewhere else. Scanned rather than imported, like the
    // UI_SCALE_MIN read further down this file.
    const rust = readFileSync(resolve(stylesDir, '../traffic_lights.rs'), 'utf-8');
    const placed = Number(/LIGHTS_X_PX:\s*f64\s*=\s*([\d.]+)/.exec(rust)?.[1]);
    expect(placed, 'LIGHTS_X_PX not found in traffic_lights.rs').toBeGreaterThan(0);

    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    const fallback = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    expect(fallback).toBe(placed);
  });

  it('the reserve still clears a control that is centred on the bar', () => {
    // The sideways step is the ONLY thing keeping the leading control off the
    // lights now, so it has to clear them for a box that reaches up into the
    // band rather than one pinned below it. --header-icon-box is a COPY of a
    // value that lives in another stylesheet (CSS cannot read a rule's own
    // declaration back out), so this also catches it drifting from the button.
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const iconBox = cssRules(readFileSync(resolve(stylesDir, 'global/host-components.css'), 'utf-8'))
      .find(r => r.selector === '.icon-btn.header-icon');
    expect(root?.props.get('--header-icon-box'), '--header-icon-box drifted from the button')
      .toBe(iconBox?.props.get('height'));
  });

  it('the drawer toggle clears the lights, and moves in no other direction', () => {
    // It moves the RESTING PLACE, not `left`: the toggle travels between two
    // positions now, and only the drawer-shut one differs between the builds.
    // Overriding `left` here would restate (and be free to drift from) the
    // drawer-open rule, which is identical on both builds.
    const rule = desktopRule(':root[data-titlebar-overlay] .thread-toggle-slot');
    expect(rule.props.get('--thread-toggle-home')).toBe('var(--titlebar-lights-reserve)');
    expect([...rule.props.keys()], 'the overlay build is horizontal-only now')
      .toEqual(['--thread-toggle-home']);
    // And the base rule is what consumes it, so the override cannot be inert.
    expect(desktopRule('.thread-toggle-slot').props.get('left'))
      .toBe('var(--thread-toggle-home)');
  });

  it('the content row keeps the reserve as a FLOOR under the divider', () => {
    // The third control that can hold the header's left-most slot: the
    // hamburger leading the Canvas pane's row. Maximizing that pane takes
    // --split-ratio to 0 and hides the drawer with it, so --divider-x is 0 and
    // the base rule lands the hamburger at the window's own left edge, on the
    // lights.
    //
    // A floor rather than a rule gated on [data-thread-collapsed], because the
    // collapse is not the only way under the reserve: --split-ratio is a
    // persisted RATIO and nothing re-clamps it when the window shrinks, so a
    // divider dropped at the Conversation pane's px floor on a very wide
    // display reopens proportionally narrower in a small one.
    const base = desktopRule('.app-header .content-header-elements').props.get('left');
    expect(base).toBe('calc(var(--divider-x) + var(--divider-width))');

    const rule = desktopRule(':root[data-titlebar-overlay] .app-header .content-header-elements');
    // Two copies of the divider term, which CSS cannot hand back as a resolved
    // length, so this is the drift check that makes the copy safe -- the same
    // shape as --threads-title-lead against the threads row's own padding
    // below. Horizontal only, like the drawer toggle above: the row keeps the
    // base rule's bar centring, so the hamburger stays on the bar's one line.
    expect(rule.props.get('left')).toBe(`max(${base}, var(--titlebar-lights-reserve))`);
    expect([...rule.props.keys()], 'horizontal-only here too').toEqual(['left']);
  });

  it('the drawer-open row starts after the lights, carrying the button with it', () => {
    // Moving the ROW rather than the button is what keeps the two drawer states
    // on one height for free: the button stays an ordinary flex child and
    // inherits the row's bar centring. The padding used to reserve the footprint
    // of a button that had left the flow, and so also carried the icon box and a
    // gap; a button in the flow occupies its own space.
    const rule = desktopRule(':root[data-titlebar-overlay] .threads-header');
    expect(rule.props.get('padding-left')).toBe('var(--titlebar-lights-reserve)');
  });

  it('the reserve survives the search field taking over the row', () => {
    // The row used to hand the reserve back under `.search-active`, on the
    // reasoning that the Filter button had gone `display: none` and nothing was
    // left up there to clear the lights. Wrong: the ROW is centred on the bar,
    // so whatever leads it reaches into the band. It was the search field, and
    // the lights sat over its magnifier and the first ~46px of the input,
    // eating any click that landed there. Nothing in this row may go back to
    // the web build's 0.5rem on this build.
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
    // The reserve resolves to 80px everywhere: it has no `--titlebar-inset` term
    // to zero it out, and its one stamped input carries a fallback so an
    // unstamped document resolves it too. So a rule keyed on a var alone would
    // indent a web header by the width of traffic lights it does not have. The
    // attribute is the only safe gate.
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
    // `flex: 1` centred it on the GAP between the Filter and Search buttons.
    // Off this build the gap's middle and the pane's coincide (the row's
    // padding is `0 0.5rem` and both controls are one --header-icon-box), so
    // nothing looked wrong; here the row starts after the lights reserve
    // instead, which put the title (reserve - 0.5rem) / 2 to the right of where
    // the drawer under it says the middle is.
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
    // The title's `flex: 1` used to push it there. With the title out of flow
    // the row has two in-flow children left, and this is what holds them apart.
    expect(desktopRule('.threads-header').props.get('justify-content')).toBe('space-between');
  });

  it('clears the WIDER of the row\'s two ends on BOTH sides, per build', () => {
    // A box centred on the pane overlaps a button as soon as it is wider than
    // twice the distance from the pane's middle to that button's inner edge, so
    // the clamp is the structural non-overlap guarantee `min-width: 0` used to
    // give the flex zone. The lead is the one term that differs between the two
    // builds, so it is the only thing the override touches; the doubling is in
    // the shared clamp and cannot drift between them.
    expect(desktopRule('.threads-header-title').props.get('max-width'))
      .toBe('calc(100% - 2 * (var(--threads-title-lead) + var(--header-icon-box) + var(--pane-header-gap)))');
    // What each build's lead actually IS is checked against the row's own
    // padding below; here the point is that the lead is the ONLY thing the
    // overlay build touches, so the doubling lives in one shared clamp and
    // cannot drift between them.
    expect([...desktopRule(':root[data-titlebar-overlay] .threads-header-title').props.keys()])
      .toEqual(['--threads-title-lead']);
  });

  it('takes its lead from the row\'s own padding, on each build', () => {
    // The clamp is only symmetric about the pane's middle if the lead it counts
    // twice is what the row actually keeps clear at its leading end. Two copies
    // of the same quantity, which CSS cannot hand back as a resolved length, so
    // this is the drift check that makes the copy safe. The overlay build's
    // pair share a var and cannot drift; the web build's are two literals.
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
    // A custom property is substituted on the element that DECLARES it, so a
    // `:root` copy of this would resolve --pane-header-gap against a rule that
    // has never heard of it (it is declared on .pane-header), making the whole
    // clamp invalid at computed-value time and silently dropping the max-width.
    for (const rule of shellRules) {
      if (rule.props.get('--threads-title-lead') === undefined) continue;
      expect(rule.selector, `${rule.selector} { --threads-title-lead }`)
        .toContain('.threads-header-title');
    }
  });
});

describe('the threads header clips sideways only, on both builds', () => {
  it('every edge is flush, now that nothing leaves the row vertically', () => {
    // Both builds used to outset an edge for a Filter button that sat outside
    // the row: the bottom for its push-down off the overlay build, the top for
    // its rise into the band on it. The button is a plain flex child again, so
    // the clip clamps at the border box on both.
    expect(desktopRule('.threads-header').props.get('clip-path')).toBe('inset(0)');
  });

  it('the overlay build adds no clip of its own', () => {
    expect(desktopRule(':root[data-titlebar-overlay] .threads-header').props.get('clip-path'))
      .toBeUndefined();
  });

  it('carries no clip release for the popout that no longer exists', () => {
    // Both builds used to carry a `:has(.thread-filter-dropdown)` release so an
    // anchored filter menu was not sheared off at the header's edge. The filter
    // is a panel in the drawer pane now (ThreadFilterPanel), nothing pops out of
    // this header, and a release keyed on a class that never renders would just
    // be a rule nobody can reach.
    expect(shellCss).not.toContain('thread-filter-dropdown');
  });

  it('nothing puts an overflow clip back on it', () => {
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
    // while --desktop-bar-height is rem, so shrinking the bar walks the overlay
    // build's header toward zero and then negative. Scanned rather than
    // imported so this stays a dependency-free source test like its neighbours.
    const prefs = readFileSync(resolve(stylesDir, '../store/actions/preferences.ts'), 'utf-8');
    const minScale = Number(/UI_SCALE_MIN\s*=\s*([\d.]+)/.exec(prefs)?.[1]);
    expect(minScale, 'UI_SCALE_MIN not found in preferences.ts').toBeGreaterThan(0);

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
    // --titlebar-inset is fixed px from the OS and --desktop-bar-height is rem,
    // so at UI_SCALE_MIN (75%) a literal 2rem would give a 52px macOS bar
    // against a 45px web one. Subtraction makes the equality structural.
    expect(desktopRule(':root[data-titlebar-overlay]').props.get('--app-header-height'))
      .toBe('calc(var(--desktop-bar-height) - var(--titlebar-inset, 0px))');
  });

  it('no mobile viewport is touched', () => {
    // "ios as is". The base token in global/base.css carries the notch's
    // env(safe-area-inset-top); every override here drops it, so one escaping
    // the desktop media query would cut the inset off an iPhone header.
    for (const rule of shellRules) {
      if (rule.props.get('--app-header-height') === undefined) continue;
      expect(rule.atRules, `${rule.selector} { --app-header-height }`).toBe(DESKTOP);
    }
  });
});
