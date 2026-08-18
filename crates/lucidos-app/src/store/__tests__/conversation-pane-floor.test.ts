/**
 * The Conversation pane's width floor, and the header clearance it buys.
 *
 * The bug this pins. On the packaged macOS build the row's drawer toggle rests
 * at the traffic-lights reserve, so the leading end is 80px plus a button. The
 * floor was a flat `18.75rem` that knew nothing about that, and
 * `--brand-side-reserve` was sized to the trailing end alone. At 125% ui-scale
 * a divider dragged to the floor put the back chevron's box 17.5px ON the
 * toggle. The web build never reproduced it: its toggle rests at 0.5rem.
 *
 * The CSS places the centred cluster and the floor says how narrow the pane
 * may get, and neither is safe alone. The clamp's min-span arm holds the
 * cluster at its natural width however narrow the pane gets. A floor under
 * that width is one the cluster overhangs. So this file resolves both from the
 * SAME tokens, at every ui-scale step, and asserts the clearance.
 *
 * A source scan, because no WebDriver reaches WKWebView (ADR 0016). The moving
 * half runs in Chromium with the build attribute stamped, in
 * `e2e/conversation-header-cluster-clearance-desktop.spec.ts`.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import {
  computeMinThreadPaneWidth, minThreadPanePx, minContentPanePx,
} from '../paneMinimums';
import { cssRules } from '../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const shellCss: string = readFileSync(
  resolve(here, '../../styles/panels/shell.css'), 'utf-8',
);

const DESKTOP = '@media (min-width: 769px)';
const desktopRoot = cssRules(shellCss).find(
  r => r.selector === ':root' && r.atRules === DESKTOP,
);

const LIGHTS_RESERVE_PX = 80;
/** The rem parts of the row, mirrored from `computeMinThreadPaneWidth`: the
 *  drawer toggle's box at each end, and the cluster's natural width between
 *  them (two chevrons and the mark's tap target). */
const ICON_BOX_REM = 2.25;
const MARK_TAP_REM = 2.1;
const CLUSTER_REM = 2 * ICON_BOX_REM + MARK_TAP_REM;
const PAD_REM = 0.5;

/** Every ui-scale step the preferences offer, as a root font size in px. */
const ROOTS = [12, 16, 18, 20, 22, 24, 28, 32];

/** The leading end of the row at a given root: where the toggle rests, plus its
 *  box. The web build rests it at the row's own padding, the packaged one at
 *  the lights reserve. Mirrors `--brand-lead-inset` and the `max()` in
 *  `computeMinThreadPaneWidth`. */
function leadPx(remPx: number, packaged: boolean): number {
  const inset = packaged ? Math.max(LIGHTS_RESERVE_PX, PAD_REM * remPx) : PAD_REM * remPx;
  return inset + ICON_BOX_REM * remPx;
}

/** `--brand-side-reserve`, resolved. The wider of the two ends, paid at both. */
function reservePx(remPx: number, packaged: boolean): number {
  const trailing = (0.5 + 3 * ICON_BOX_REM + 2 * 0.25) * remPx;
  return Math.max(leadPx(remPx, packaged), trailing);
}

describe('computeMinThreadPaneWidth', () => {
  it('is symmetric around the centred cluster, paying the lead at both ends', () => {
    // The cluster is centred on the PANE (`.pane-header-brand-label`), not on
    // the gap between the clusters flanking it. So the floor is
    // `2 * side + cluster`, and whatever leads the row is paid on both sides.
    expect(computeMinThreadPaneWidth(16, LIGHTS_RESERVE_PX))
      .toBe(Math.ceil(2 * (LIGHTS_RESERVE_PX + ICON_BOX_REM * 16) + CLUSTER_REM * 16));
    expect(computeMinThreadPaneWidth(16, LIGHTS_RESERVE_PX)).toBe(338);
  });

  it('scales with the root font size, because the row does', () => {
    // The old `18.75rem` ignored the lights, and it was also a bare multiple of
    // the root, so it tracked no part of the row. Here the rem terms scale and
    // the px lead does not. That makes the floor tighter at large scales and
    // wider at small ones.
    expect(computeMinThreadPaneWidth(20, LIGHTS_RESERVE_PX)).toBe(382);
    expect(computeMinThreadPaneWidth(24, LIGHTS_RESERVE_PX)).toBe(427);
    expect(computeMinThreadPaneWidth(28, LIGHTS_RESERVE_PX)).toBe(471);
  });

  it('floors the lead at the row\'s own padding, for a lead that is not there', () => {
    // The `max` keeps it honest at the other end of the scale, exactly as the
    // drawer's does: at a large enough root the rem padding is the wider end.
    expect(computeMinThreadPaneWidth(16, 0))
      .toBe(Math.ceil(2 * (PAD_REM * 16 + ICON_BOX_REM * 16) + CLUSTER_REM * 16));
  });

  it('is ONE floor for every desktop client', () => {
    // The caller passes the lights reserve on the web build too, the same call
    // `computeMinDrawerWidth` makes (ADR 0058): a workspace stops the divider at
    // the same width in the browser as in the app. `data-titlebar-overlay`
    // decides how the row is LAID OUT, never how narrow the pane may get.
    const web = minThreadPanePx();
    document.documentElement.setAttribute('data-titlebar-overlay', '');
    try {
      expect(minThreadPanePx()).toBe(web);
    } finally {
      document.documentElement.removeAttribute('data-titlebar-overlay');
    }
  });
});

describe('the centred cluster cannot reach a flanking control', () => {
  afterEach(() => vi.unstubAllGlobals());

  const floorAt = (remPx: number) => computeMinThreadPaneWidth(remPx, LIGHTS_RESERVE_PX);

  it.each(ROOTS)('clears both ends at a %ipx root, on both builds', (remPx) => {
    for (const packaged of [true, false]) {
      const lead = leadPx(remPx, packaged);
      const reserve = reservePx(remPx, packaged);
      const floor = floorAt(remPx);

      // The clamp's middle arm. `--brand-side-reserve` IS the box's leading
      // edge there, so it has to be at least the leading end.
      expect(reserve, `middle arm at ${remPx}px, packaged=${packaged}`)
        .toBeGreaterThanOrEqual(lead);

      // The clamp's min-span arm, at its worst width: the pane's own floor.
      // Below this the box would overhang, which is what the floor buys.
      expect((floor - CLUSTER_REM * remPx) / 2, `min arm at ${remPx}px, packaged=${packaged}`)
        .toBeGreaterThanOrEqual(lead);
    }
  });

  it('the floor is exactly where the two arms meet, not a padded guess', () => {
    // Equality, not slack. The floor IS the width at which the cluster, held at
    // its natural span, exactly meets the toggle. A change to either side
    // therefore moves both together. Slack here would read as a fudge factor,
    // and would drift the moment someone trimmed it.
    for (const remPx of ROOTS) {
      const want = CLUSTER_REM * remPx + 2 * leadPx(remPx, true);
      expect(floorAt(remPx) - want, `slack at ${remPx}px`).toBeLessThan(1);
    }
  });

  it('the Canvas row needs no floor change, since its own already covers it', () => {
    // The two centred desktop clusters share `--desktop-nav-min-span`, so
    // lowering it to the brand cluster's natural width reaches the Canvas row
    // too. It is harmless there: `2 * --content-side-reserve + the min span` is
    // 22.1rem, inside `MIN_CONTENT_PANE_REM`, so that row sits on the clamp's
    // middle arm at its own floor and never meets the lowered value.
    const CONTENT_SIDE_RESERVE_REM = 3 * ICON_BOX_REM + 4 * 0.25;
    for (const remPx of ROOTS) {
      vi.stubGlobal('getComputedStyle', () => ({
        fontSize: `${remPx}px`, getPropertyValue: () => '',
      }));
      expect(2 * CONTENT_SIDE_RESERVE_REM * remPx + CLUSTER_REM * remPx, `at ${remPx}px`)
        .toBeLessThanOrEqual(minContentPanePx());
    }
  });
});

describe('the TS mirror of the row still matches the CSS', () => {
  // `computeMinThreadPaneWidth` adds up quantities DECLARED in shell.css, and
  // CSS cannot hand them back as a resolved length, so the sum is a copy. These
  // are the drift checks that make the copy safe. The SHAPE of each expression
  // is pinned next door, in styles/__tests__/header-band-centering.test.ts.

  it('the icon box is what the floor counts three times', () => {
    // Once at each end, and twice inside the cluster.
    expect(desktopRoot?.props.get('--header-icon-box')).toBe(`${ICON_BOX_REM}rem`);
  });

  it('the mark\'s tap target is what the cluster holds between the chevrons', () => {
    const markCss = readFileSync(resolve(here, '../../styles/header-mark.css'), 'utf-8');
    const markRoot = cssRules(markCss).find(r => r.selector === ':root');
    expect(markRoot?.props.get('--header-mark-tap')).toBe(`${MARK_TAP_REM}rem`);
  });

  it('the web build rests the toggle at the padding the floor falls back to', () => {
    expect(desktopRoot?.props.get('--brand-lead-inset')).toBe(`${PAD_REM}rem`);
  });

  it('the packaged build rests it at the reserve the floor is sized around', () => {
    const overlay = cssRules(shellCss).find(
      r => r.selector === ':root[data-titlebar-overlay]' && r.atRules === DESKTOP,
    );
    expect(overlay?.props.get('--brand-lead-inset')).toBe('var(--titlebar-lights-reserve)');
    // And the sum that reserve resolves to is the one this file restates.
    const reserve = desktopRoot!.props.get('--titlebar-lights-reserve')!;
    const x = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    const cluster = parseInt(desktopRoot!.props.get('--titlebar-lights-cluster')!, 10);
    const gap = parseInt(desktopRoot!.props.get('--titlebar-lights-gap')!, 10);
    expect(x + cluster + gap).toBe(LIGHTS_RESERVE_PX);
  });

  it('the clamp floors the box at the same cluster width the floor reserves', () => {
    // The load-bearing pairing. The min-span arm decides how wide the box is at
    // a narrow pane, and the floor decides how narrow the pane gets. A literal
    // in either one is what let them describe different rows.
    expect(desktopRoot?.props.get('--desktop-nav-min-span'))
      .toBe('calc(2 * var(--header-icon-box) + var(--header-mark-tap))');
    const label = cssRules(shellCss).find(
      r => r.selector === '.app-header .pane-header-brand .pane-header-brand-label'
        && r.atRules === DESKTOP,
    );
    expect(label?.props.get('width')).toContain('var(--desktop-nav-min-span)');
    expect(label?.props.get('width')).toContain('var(--brand-side-reserve)');
  });
});
