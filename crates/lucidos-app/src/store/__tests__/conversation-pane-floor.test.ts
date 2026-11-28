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

const DESKTOP = '@media (--desktop-layout)';
const desktopRoot = cssRules(shellCss).find(
  r => r.selector === ':root' && r.atRules === DESKTOP,
);

const LIGHTS_RESERVE_PX = 80;
const ICON_BOX_REM = 2.25;
const MARK_TAP_REM = 2.1;
const CLUSTER_REM = 2 * ICON_BOX_REM + MARK_TAP_REM;
const PAD_REM = 0.5;

const ROOTS = [12, 16, 18, 20, 22, 24, 28, 32];

function leadPx(remPx: number, packaged: boolean): number {
  const inset = packaged ? Math.max(LIGHTS_RESERVE_PX, PAD_REM * remPx) : PAD_REM * remPx;
  return inset + ICON_BOX_REM * remPx;
}

function reservePx(remPx: number, packaged: boolean): number {
  const trailing = (0.5 + 2 * ICON_BOX_REM + 0.25) * remPx;
  return Math.max(leadPx(remPx, packaged), trailing);
}

describe('computeMinThreadPaneWidth', () => {
  it('is symmetric around the centred cluster, paying the lead at both ends', () => {
    expect(computeMinThreadPaneWidth(16, LIGHTS_RESERVE_PX, 0))
      .toBe(Math.ceil(2 * (LIGHTS_RESERVE_PX + ICON_BOX_REM * 16) + CLUSTER_REM * 16));
    expect(computeMinThreadPaneWidth(16, LIGHTS_RESERVE_PX, 0)).toBe(338);
  });

  it('scales with the root font size, because the row does', () => {
    expect(computeMinThreadPaneWidth(20, LIGHTS_RESERVE_PX, 0)).toBe(382);
    expect(computeMinThreadPaneWidth(24, LIGHTS_RESERVE_PX, 0)).toBe(427);
    expect(computeMinThreadPaneWidth(28, LIGHTS_RESERVE_PX, 0)).toBe(471);
  });

  it('floors the lead at the row\'s own padding, for a lead that is not there', () => {
    expect(computeMinThreadPaneWidth(16, 0, 0))
      .toBe(Math.ceil(2 * (PAD_REM * 16 + ICON_BOX_REM * 16) + CLUSTER_REM * 16));
  });

  it('pays a box and a gap at both ends for each control leading the row', () => {
    const gapRem = 0.25;
    const bare = computeMinThreadPaneWidth(16, LIGHTS_RESERVE_PX, 0);
    expect(computeMinThreadPaneWidth(16, LIGHTS_RESERVE_PX, 1)).toBe(bare + 2 * (ICON_BOX_REM + gapRem) * 16);
    expect(computeMinThreadPaneWidth(16, LIGHTS_RESERVE_PX, 2)).toBe(bare + 4 * (ICON_BOX_REM + gapRem) * 16);
  });

  it('is ONE floor for every desktop client', () => {
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

  const floorAt = (remPx: number) => computeMinThreadPaneWidth(remPx, LIGHTS_RESERVE_PX, 0);

  it.each(ROOTS)('clears both ends at a %ipx root, on both builds', (remPx) => {
    for (const packaged of [true, false]) {
      const lead = leadPx(remPx, packaged);
      const reserve = reservePx(remPx, packaged);
      const floor = floorAt(remPx);

      expect(reserve, `middle arm at ${remPx}px, packaged=${packaged}`)
        .toBeGreaterThanOrEqual(lead);

      expect((floor - CLUSTER_REM * remPx) / 2, `min arm at ${remPx}px, packaged=${packaged}`)
        .toBeGreaterThanOrEqual(lead);
    }
  });

  it('the floor is exactly where the two arms meet, not a padded guess', () => {
    for (const remPx of ROOTS) {
      const want = CLUSTER_REM * remPx + 2 * leadPx(remPx, true);
      expect(floorAt(remPx) - want, `slack at ${remPx}px`).toBeLessThan(1);
    }
  });

  it('the Canvas floor covers the undocked control reserve', () => {
    const CONTENT_SIDE_RESERVE_REM = 2 * ICON_BOX_REM + 3 * 0.25;
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

  it('the icon box is what the floor counts three times', () => {
    expect(desktopRoot?.props.get('--header-icon-box')).toBe(`${ICON_BOX_REM}rem`);
  });

  it('the mark\'s tap target is what the cluster holds between the chevrons', () => {
    const markCss = readFileSync(resolve(here, '../../styles/header-mark.css'), 'utf-8');
    const markRoot = cssRules(markCss).find(r => r.selector === ':root');
    expect(markRoot?.props.get('--header-mark-tap')).toBe(`${MARK_TAP_REM}rem`);
  });

  it('the web build rests the toggle at the padding the floor falls back to', () => {
    expect(desktopRoot?.props.get('--header-lead-inset')).toBe(`${PAD_REM}rem`);
  });

  it('the packaged build rests it at the reserve the floor is sized around', () => {
    const overlay = cssRules(shellCss).find(
      r => r.selector === ':root[data-titlebar-overlay]' && r.atRules === DESKTOP,
    );
    expect(overlay?.props.get('--header-lead-inset')).toBe('var(--titlebar-lights-reserve)');
    const reserve = desktopRoot!.props.get('--titlebar-lights-reserve')!;
    const x = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    const cluster = parseInt(desktopRoot!.props.get('--titlebar-lights-cluster')!, 10);
    const gap = parseInt(desktopRoot!.props.get('--titlebar-lights-gap')!, 10);
    expect(x + cluster + gap).toBe(LIGHTS_RESERVE_PX);
  });

  it('the clamp floors the box at the same cluster width the floor reserves', () => {
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
