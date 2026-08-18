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
 * WIDER of the row's two ends on BOTH sides, so the reserve is now paid twice.
 * Then it turned build-independent: the web client pays the same reserve, so a
 * workspace stops the drawer at the same width in the browser as in the app.
 *
 * Three things are pinned: the arithmetic at both roots and at both leads, the
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

/** The rem parts of the row, mirrored from `computeMinDrawerWidth`'s constants:
 *  a button and a gap at each end, and the title between them. */
const ROW_REM = 2 * (2.25 + 0.25) + 4.5;
/** The row's own padding, per end. */
const PAD_REM = 0.5;
const LIGHTS_RESERVE_PX = 80;

describe('computeMinDrawerWidth', () => {
  it('is symmetric around the centred title, paying the lead at both ends', () => {
    // The title is centred on the PANE (`.threads-header-title`), not on the gap
    // between the two buttons, so the floor is `2 * side + title` rather than a
    // single run of controls, and whatever leads the row is paid on both sides.
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX))
      .toBe(Math.ceil(2 * LIGHTS_RESERVE_PX + ROW_REM * 16));
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX)).toBe(312);
  });

  it('scales with the root font size, because the row does', () => {
    // The whole reason the constant had to go: at 125% the same controls need
    // 25% more room, and a px literal does not know that.
    expect(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX))
      .toBe(Math.ceil(2 * LIGHTS_RESERVE_PX + ROW_REM * 20));
    expect(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX))
      .toBeGreaterThan(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX));
  });

  it('is what the row costs with only its own padding, plus the lead twice over', () => {
    // The web row lays out at 0.5rem, and the floor it is held to is the
    // packaged build's: the difference between the two is `2 * (reserve -
    // padding)`, 144px at a 16px root, and it is title room the web row gains
    // rather than anything it has to clear.
    const rowsOwnPadding = Math.ceil(2 * PAD_REM * 16 + ROW_REM * 16);
    expect(rowsOwnPadding).toBe(168);
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX))
      .toBe(rowsOwnPadding + 2 * (LIGHTS_RESERVE_PX - PAD_REM * 16));
  });

  it('never lets the fixed reserve shrink an end below the row\'s own padding', () => {
    // The reserve is px and the padding is rem, so at a large enough root the
    // padding is the wider end and the max() in the floor is what picks it. A
    // plain "take the reserve" lead would narrow the drawer as the UI scaled up,
    // which is backwards.
    expect(computeMinDrawerWidth(200, LIGHTS_RESERVE_PX))
      .toBe(Math.ceil(2 * PAD_REM * 200 + ROW_REM * 200));
  });

  it('exceeds the retired 260px constant exactly where the bug was reported', () => {
    // 125% ui-scale on the packaged build. The old floor let the drawer rest
    // there with its header overflowing.
    expect(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX)).toBeGreaterThan(260);
  });

  it('holds the reserve fixed while the rem part scales', () => {
    // The lights are OS chrome: they do not grow with our root font size, so
    // doubling the root must double only the rem term.
    const grew = computeMinDrawerWidth(32, LIGHTS_RESERVE_PX)
      - computeMinDrawerWidth(16, LIGHTS_RESERVE_PX);
    expect(grew).toBe(ROW_REM * 16);
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
    // decides how the row LAYS OUT (`:root[data-titlebar-overlay]
    // .threads-header` takes the reserve as its padding-left); it decides
    // nothing about how narrow the drawer may get.
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

  it('the icon box is 2.25rem, counted twice in the row', () => {
    expect(desktopRoot?.props.get('--header-icon-box')).toBe('2.25rem');
  });

  it('the row gap is 0.25rem, counted twice in the row', () => {
    expect(paneHeader?.props.get('--pane-header-gap')).toBe('0.25rem');
  });

  it('the row padding is 0 0.5rem, half leading and half trailing', () => {
    const row = rules.find(r => r.selector === '.threads-header' && r.atRules === DESKTOP);
    expect(row?.props.get('padding')).toBe('0 0.5rem');
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

  it('the overlay build reserves exactly the lead the floor assumes', () => {
    // The floor says "lights reserve, then the row"; the CSS has to actually
    // keep that much clear, or the two describe different rows. It used to say
    // "reserve + icon box + gap", because the Filter button was absolutely
    // positioned out of the flow and the padding stood in for its footprint.
    const overlayRow = rules.find(
      r => r.selector === ':root[data-titlebar-overlay] .threads-header' && r.atRules === DESKTOP,
    );
    expect(overlayRow?.props.get('padding-left')).toBe('var(--titlebar-lights-reserve)');
  });

  it('the centred title clamps to the same lead the floor pays twice', () => {
    // The floor is `2 * (lead + button + gap) + title`, and the doubling is
    // there because the title is centred on the PANE. The CSS clamp has to
    // count the same lead twice or the two disagree about the row: a clamp that
    // paid it once would let the title run under the Filter button, and a floor
    // that paid it once would stop the drawer at a width where the title it
    // reserved 4.5rem for is an ellipsis. The exact clamp expression is pinned
    // next door, in styles/__tests__/header-band-centering.test.ts.
    const title = rules.find(
      r => r.selector === '.threads-header-title' && r.atRules === DESKTOP,
    );
    expect(title?.props.get('max-width')).toContain('2 * (var(--threads-title-lead)');
    // …and on the overlay build that lead IS the reserve, which is what the
    // floor's `leadPx` argument carries on every build.
    const overlayTitle = rules.find(
      r => r.selector === ':root[data-titlebar-overlay] .threads-header-title'
        && r.atRules === DESKTOP,
    );
    expect(overlayTitle?.props.get('--threads-title-lead'))
      .toBe('var(--titlebar-lights-reserve)');
  });
});
