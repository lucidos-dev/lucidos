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
 * The web build is unaffected (its two ends were always the same 0.5rem).
 *
 * Three things are pinned: the arithmetic at both roots and on both builds, the
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
  it('is symmetric around the centred title, both ends being the row\'s padding', () => {
    // The title is centred on the PANE (`.threads-header-title`), not on the gap
    // between the two buttons, so the floor is `2 * side + title` rather than a
    // single run of controls. On the web build both ends are the same 0.5rem, so
    // this is byte-identical to the pre-centring floor of 168.
    expect(computeMinDrawerWidth(16, null)).toBe(Math.ceil(2 * PAD_REM * 16 + ROW_REM * 16));
    expect(computeMinDrawerWidth(16, null)).toBe(168);
  });

  it('scales with the root font size, because the row does', () => {
    // The whole reason the constant had to go: at 125% the same controls need
    // 25% more room, and a px literal does not know that.
    expect(computeMinDrawerWidth(20, null)).toBe(Math.ceil(2 * PAD_REM * 20 + ROW_REM * 20));
    expect(computeMinDrawerWidth(20, null)).toBeGreaterThan(computeMinDrawerWidth(16, null));
  });

  it('pays the traffic-lights reserve at BOTH ends on the packaged macOS build', () => {
    // There the row's leading edge is the fixed OS reserve rather than our
    // 0.5rem padding, because its controls reach up into the reclaimed band and
    // would otherwise paint over a light. A title centred on the pane has to
    // clear the WIDER end on both sides, so the reserve is counted twice: the
    // whole difference from the web floor is `2 * (reserve - padding)`.
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX))
      .toBe(Math.ceil(2 * LIGHTS_RESERVE_PX + ROW_REM * 16));
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX))
      .toBe(computeMinDrawerWidth(16, null) + 2 * (LIGHTS_RESERVE_PX - PAD_REM * 16));
  });

  it('never lets the fixed reserve shrink an end below the row\'s own padding', () => {
    // The reserve is px and the padding is rem, so at a large enough root the
    // padding is the wider end and the max() in the floor is what picks it. A
    // plain "reserve or padding" lead would narrow the drawer as the UI scaled
    // up, which is backwards.
    expect(computeMinDrawerWidth(200, LIGHTS_RESERVE_PX)).toBe(computeMinDrawerWidth(200, null));
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

  it('answers the web-build floor with no overlay attribute stamped', () => {
    // The test harness has no layout engine, so this exercises the fallback
    // path: a 16px root and no reserve property to read.
    expect(minDrawerWidth()).toBe(computeMinDrawerWidth(16, null));
  });

  it('tracks the live root font size', () => {
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '20px', getPropertyValue: () => '' }));
    expect(minDrawerWidth()).toBe(computeMinDrawerWidth(20, null));
  });

  it('adds the reserve, read from CSS, once the overlay attribute is stamped', () => {
    vi.stubGlobal('getComputedStyle', () => ({
      fontSize: '16px',
      getPropertyValue: (name: string) => (name === '--titlebar-lights-reserve' ? '90px' : ''),
    }));
    const root = document.documentElement;
    const had = root.hasAttribute;
    root.hasAttribute = (name: string) => name === 'data-titlebar-overlay' || had.call(root, name);
    try {
      // 90, not the 80px literal: the property is the source, the literal is
      // only the fallback for when it cannot be read.
      expect(minDrawerWidth()).toBe(computeMinDrawerWidth(16, 90));
    } finally {
      root.hasAttribute = had;
    }
  });

  it('falls back to the px literal when the property is unreadable', () => {
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '16px', getPropertyValue: () => '' }));
    const root = document.documentElement;
    const had = root.hasAttribute;
    root.hasAttribute = (name: string) => name === 'data-titlebar-overlay' || had.call(root, name);
    try {
      expect(minDrawerWidth()).toBe(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX));
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

  it('equal the px constants they replace at a 16px root', () => {
    // Derived, not re-tuned: a default-scale layout must be untouched by the
    // move off `MIN_THREAD_PANE_PX = 300` / `MIN_CONTENT_PANE_PX = 360`.
    expect(minThreadPanePx()).toBe(300);
    expect(minContentPanePx()).toBe(360);
  });

  it('scale with the root, which is the whole point of deriving them', () => {
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '20px', getPropertyValue: () => '' }));
    expect(minThreadPanePx()).toBe(375);
    expect(minContentPanePx()).toBe(450);
  });

  it('stop fitting a 1280px screen past ~150% ui-scale, which the clamp must handle', () => {
    // Not a hypothetical: this is the configuration that makes
    // `clampToRange`'s empty-range branch load-bearing rather than defensive.
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '28px', getPropertyValue: () => '' }));
    const sum = minDrawerWidth() + minThreadPanePx() + minContentPanePx();
    expect(sum).toBeGreaterThan(1280);
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

  it('the lights reserve is the px value the fallback restates', () => {
    expect(desktopRoot?.props.get('--titlebar-lights-reserve')).toBe(`${LIGHTS_RESERVE_PX}px`);
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
    // …and on the overlay build that lead IS the reserve, which is the half the
    // floor's `lightsReservePx` argument stands for.
    const overlayTitle = rules.find(
      r => r.selector === ':root[data-titlebar-overlay] .threads-header-title'
        && r.atRules === DESKTOP,
    );
    expect(overlayTitle?.props.get('--threads-title-lead'))
      .toBe('var(--titlebar-lights-reserve)');
  });
});
