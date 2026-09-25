/**
 * The thread drawer's width floor.
 *
 * It used to be `MIN_DRAWER_WIDTH = 260`, a px constant computed once "at
 * 16px/rem" for a five-button drawer header that no longer exists. Every part of
 * the row it sizes is rem-authored, so the constant was only ever right at one
 * UI scale, and wrong again on the packaged macOS build, where the row starts
 * after a fixed 80px reserve that clears the traffic lights. At 125% on that
 * build the row needs ~280px and the old floor let a drag rest at 260, which is
 * the header overflowing its own drawer.
 *
 * The floor turned symmetric when the Threads title moved off the gap between
 * its two buttons and onto the pane's own middle: a centred title clears the
 * WIDER of the row's two ends on BOTH sides, so the wider end is paid twice.
 * Then it turned build-independent: the web client pays the same reserve, so a
 * workspace stops the drawer at the same width in the browser as in the app.
 *
 * The two ends are the lead plus the drawer toggle, which rests over the row,
 * and the padding plus Filter and Search. The first is the wider one up to
 * about 166% ui-scale, because the lead is a px reserve and the rest is rem.
 *
 * Three things are pinned: the arithmetic on both sides of that crossing, the
 * re-clamp a UI-scale change owes a settled drawer, and the fact that the TS
 * mirror of the row's rem parts still matches the CSS declaring them.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { clampThreadDrawerWidth, threadDrawerWidth, THREAD_DRAWER_WIDTH_KEY } from '../store';
import {
  computeMinDrawerWidth, minDrawerWidth, minThreadPanePx, minContentPanePx,
} from '../paneMinimums';
import { cssRules } from '../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const shellCss: string = readFileSync(
  resolve(here, '../../styles/panels/shell.css'), 'utf-8',
);

/** A button and its gap, mirrored from `computeMinDrawerWidth`'s constants. */
const BUTTON_REM = 2.25 + 0.25;
/** The title's own room. */
const TITLE_REM = 4.5;
/** The row's own trailing padding. */
const PAD_REM = 0.5;
const LIGHTS_RESERVE_PX = 80;

/** The row's leading end at a root: the lead, then the drawer toggle. */
const leadEnd = (rem: number, lead = LIGHTS_RESERVE_PX) => lead + BUTTON_REM * rem;
/** The row's trailing end at a root: the padding, then Filter and Search. */
const trailEnd = (rem: number) => (PAD_REM + 2 * BUTTON_REM) * rem;

describe('computeMinDrawerWidth', () => {
  it('is symmetric around the centred title, paying the wider end at both sides', () => {
    // The title is centred on the PANE (`.threads-header-title`), not on the gap
    // between the controls, so the floor is `2 * side + title` rather than a
    // single run of controls, and the wider end is paid on both sides.
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX))
      .toBe(Math.ceil(2 * leadEnd(16) + TITLE_REM * 16));
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX)).toBe(312);
  });

  it('is sized by the leading end through the everyday scales', () => {
    // 75% to 162.5%: the px reserve plus the toggle outweighs Filter and Search.
    for (const rem of [12, 16, 20, 22, 24, 26]) {
      expect(leadEnd(rem), `${rem}px root`).toBeGreaterThan(trailEnd(rem));
      expect(computeMinDrawerWidth(rem, LIGHTS_RESERVE_PX), `${rem}px root`)
        .toBe(Math.ceil(2 * leadEnd(rem) + TITLE_REM * rem));
    }
  });

  it('is sized by the trailing end above about 166% ui-scale', () => {
    // Filter and Search are all rem, the reserve is not, so from 175% up the
    // trailing end is the wider one. A floor that kept paying the leading end
    // there would let the title run under Filter.
    for (const rem of [28, 32]) {
      expect(trailEnd(rem), `${rem}px root`).toBeGreaterThan(leadEnd(rem));
      expect(computeMinDrawerWidth(rem, LIGHTS_RESERVE_PX), `${rem}px root`)
        .toBe(Math.ceil(2 * trailEnd(rem) + TITLE_REM * rem));
    }
    expect(computeMinDrawerWidth(28, LIGHTS_RESERVE_PX)).toBe(434);
  });

  it('scales with the root font size, because the row does', () => {
    // The whole reason the constant had to go: at 125% the same controls need
    // 25% more room, and a px literal does not know that.
    expect(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX))
      .toBeGreaterThan(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX));
  });

  it('holds the web row to the packaged build\'s floor, as title room it gains', () => {
    // The web row leads with the toggle at 0.5rem, so its own need is the
    // wider of that end and the trailing one. The floor it is held to is the
    // packaged build's (ADR 0058). The difference is title room the web row
    // gains rather than anything it has to clear: 64px at a 16px root.
    const webRow = Math.ceil(2 * Math.max(leadEnd(16, PAD_REM * 16), trailEnd(16)) + TITLE_REM * 16);
    expect(webRow).toBe(248);
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX) - webRow)
      .toBe(2 * (leadEnd(16) - trailEnd(16)));
  });

  it('never stops the drawer narrower than the web row needs, at any root', () => {
    // The floor is paid with the packaged build's lead on every client. It
    // must still cover the web row wherever the rem part outgrows the reserve.
    for (const rem of [12, 16, 24, 32, 48, 200]) {
      const webRow = Math.ceil(2 * Math.max(leadEnd(rem, PAD_REM * rem), trailEnd(rem)) + TITLE_REM * rem);
      expect(computeMinDrawerWidth(rem, LIGHTS_RESERVE_PX), `${rem}px root`)
        .toBeGreaterThanOrEqual(webRow);
    }
  });

  it('exceeds the retired 260px constant exactly where the bug was reported', () => {
    // 125% ui-scale on the packaged build. The old floor let the drawer rest
    // there with its header overflowing.
    expect(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX)).toBeGreaterThan(260);
  });

  it('holds the reserve fixed while the rem part scales', () => {
    // The lights are OS chrome: they do not grow with our root font size, so
    // growing the root grows only the rem term. 16px to 24px stays on the
    // leading end, where the reserve is paid.
    const grew = computeMinDrawerWidth(24, LIGHTS_RESERVE_PX)
      - computeMinDrawerWidth(16, LIGHTS_RESERVE_PX);
    expect(grew).toBe((2 * BUTTON_REM + TITLE_REM) * 8);
  });
});

describe('minDrawerWidth reads the live root', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('reads the reserve from CSS', () => {
    vi.stubGlobal('getComputedStyle', () => ({
      fontSize: '16px',
      getPropertyValue: (name: string) => (name === '--titlebar-lights-reserve' ? '90px' : ''),
    }));
    // 90, not the 80px literal: the property is the source, the literal is only
    // the fallback for when it cannot be read.
    expect(minDrawerWidth()).toBe(computeMinDrawerWidth(16, 90));
  });

  it('falls back to the px literal when the property is unreadable', () => {
    // The harness has no layout engine, so this is also the default path here.
    expect(minDrawerWidth()).toBe(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX));
  });

  it('tracks the live root font size', () => {
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '20px', getPropertyValue: () => '' }));
    expect(minDrawerWidth()).toBe(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX));
  });

  it('answers the same floor whether or not the build stamps the overlay', () => {
    // The user's ask, and the one property that would silently rot if the
    // attribute crept back into the floor: a workspace has to stop the drawer at
    // the same width in the browser as in the packaged app. The attribute still
    // decides how the row LAYS OUT (it moves `--header-lead-inset`, which the
    // row's lead reads); it decides nothing about how narrow the drawer may get.
    const web = minDrawerWidth();
    const root = document.documentElement;
    const had = root.hasAttribute;
    root.hasAttribute = (name: string) => name === 'data-titlebar-overlay' || had.call(root, name);
    try {
      expect(minDrawerWidth()).toBe(web);
    } finally {
      root.hasAttribute = had;
    }
  });
});

describe('clampThreadDrawerWidth', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('widens a drawer left under the floor, and persists the correction', () => {
    const min = minDrawerWidth();
    threadDrawerWidth.value = min - 40;
    clampThreadDrawerWidth();
    expect(threadDrawerWidth.value).toBe(min);
    // Persisted, so the next boot starts corrected instead of re-correcting.
    expect(localStorage.getItem(THREAD_DRAWER_WIDTH_KEY)).toBe(String(min));
  });

  it('leaves a drawer that already clears the floor alone', () => {
    threadDrawerWidth.value = 600;
    clampThreadDrawerWidth();
    expect(threadDrawerWidth.value).toBe(600);
    expect(localStorage.getItem(THREAD_DRAWER_WIDTH_KEY)).toBeNull();
  });
});

describe('the two split-pane floors', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('are what their own rows need at a 16px root', () => {
    // The Canvas floor is still the 360px constant it always was. The
    // Conversation one is not: it was 300, and its row needs 338, which is the
    // gap the reported header overlap fell through. See
    // `conversation-pane-floor.test.ts` for where the 338 comes from.
    expect(minThreadPanePx()).toBe(338);
    expect(minContentPanePx()).toBe(360);
  });

  it('scale with the root, which is the whole point of deriving them', () => {
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '20px', getPropertyValue: () => '' }));
    expect(minThreadPanePx()).toBe(382);
    expect(minContentPanePx()).toBe(450);
  });

  it('stop fitting a 1280px screen from 150% ui-scale, which the clamp must handle', () => {
    // Not a hypothetical: this is the configuration that makes
    // `clampToRange`'s empty-range branch load-bearing rather than defensive.
    // The crossing moved down a step when the drawer's floor became the
    // packaged build's on every client, so pin both sides of it: 137.5% is the
    // widest scale that still fits, 150% the first that does not.
    const sumAt = (fontSize: string) => {
      vi.stubGlobal('getComputedStyle', () => ({ fontSize, getPropertyValue: () => '' }));
      return minDrawerWidth() + minThreadPanePx() + minContentPanePx();
    };
    expect(sumAt('22px')).toBeLessThan(1280);
    expect(sumAt('24px')).toBeGreaterThan(1280);
  });
});

describe('the TS mirror of the row still matches the CSS', () => {
  // `computeMinDrawerWidth` adds up quantities DECLARED in shell.css; CSS cannot
  // hand them back as a resolved length, so the sum is a copy. These are the
  // drift checks that make the copy safe.
  const rules = cssRules(shellCss);
  const DESKTOP = '@media (min-width: 769px)';
  const desktopRoot = rules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
  const paneHeader = rules.find(r => r.selector === '.pane-header');

  const row = rules.find(r => r.selector === '.threads-header' && r.atRules === DESKTOP);

  it('the icon box is 2.25rem: the toggle, Filter and Search', () => {
    expect(desktopRoot?.props.get('--header-icon-box')).toBe('2.25rem');
  });

  it('the row gap is 0.25rem, one per button', () => {
    expect(paneHeader?.props.get('--pane-header-gap')).toBe('0.25rem');
  });

  it('the row pads 0.5rem at its trailing end and the toggle\'s room at its leading one', () => {
    expect(row?.props.get('padding')).toBe(`0 ${PAD_REM}rem 0 var(--threads-row-lead)`);
    // One button and one gap past the lead: the toggle, which rests over the
    // row. That is `leadEnd`, which the floor pays with the lights reserve.
    expect(row?.props.get('--threads-row-lead'))
      .toBe('calc(var(--header-lead-inset) + var(--header-icon-box) + var(--pane-header-gap))');
  });

  it('the lights reserve still SUMS to the px value the fallback restates', () => {
    // The CSS no longer states the reserve as a literal: it is the x the shell
    // places the traffic lights at (stamped pre-paint, with a fallback here),
    // plus the cluster's measured width, plus what is left over. What
    // `paneMinimums.ts` restates is the SUM, which is what this checks, because
    // the sum is what a viewport too narrow to declare the property falls back
    // to. The shape of the arithmetic is pinned next door, in
    // styles/__tests__/header-band-centering.test.ts.
    const reserve = desktopRoot!.props.get('--titlebar-lights-reserve')!;
    const x = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    expect(x, 'the reserve no longer derives from a stamped x').toBeGreaterThan(0);
    const cluster = parseInt(desktopRoot!.props.get('--titlebar-lights-cluster')!, 10);
    const gap = parseInt(desktopRoot!.props.get('--titlebar-lights-gap')!, 10);
    expect(x + cluster + gap).toBe(LIGHTS_RESERVE_PX);
  });

  it('the overlay build leads with exactly the reserve the floor assumes', () => {
    // The floor says "lights reserve, then the toggle"; the CSS has to keep
    // that much clear, or the two describe different rows. The overlay build
    // moves the toggle's inset, and the row's lead reads the same inset.
    const overlayRoot = rules.find(
      r => r.selector === ':root[data-titlebar-overlay]' && r.atRules === DESKTOP,
    );
    expect(overlayRoot?.props.get('--header-lead-inset')).toBe('var(--titlebar-lights-reserve)');
  });

  it('the centred title clamps to the same two ends the floor pays twice', () => {
    // The floor is `2 * max(leadEnd, trailEnd) + title`, and the doubling is
    // there because the title is centred on the PANE. The CSS clamp has to
    // count the same two ends, or the two disagree about the row. Skip one in
    // the clamp and the title runs under a control. Skip one in the floor and
    // the drawer stops where the title is an ellipsis. The exact expression is
    // pinned next door, in styles/__tests__/header-band-centering.test.ts.
    const title = rules.find(
      r => r.selector === '.threads-header-title' && r.atRules === DESKTOP,
    );
    const clamp = title?.props.get('max-width') ?? '';
    expect(clamp).toContain('2 * max(var(--threads-row-lead),');
    expect(clamp).toContain(`${PAD_REM}rem + 2 * (var(--header-icon-box) + var(--pane-header-gap))`);
  });
});
