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

const BUTTON_REM = 2.25 + 0.25;
const PAD_REM = 0.5;
const LIGHTS_RESERVE_PX = 80;

const leadEnd = (rem: number, lead = LIGHTS_RESERVE_PX) => lead + 2 * BUTTON_REM * rem;
const trailEnd = (rem: number) => (PAD_REM + 2 * BUTTON_REM) * rem;

describe('computeMinDrawerWidth', () => {
  it('is symmetric around the centred title, paying the wider end at both sides', () => {
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX)).toBe(Math.ceil(2 * leadEnd(16)));
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX)).toBe(320);
  });

  it('is sized by the leading end at every scale a user can pick', () => {
    for (const rem of [12, 16, 20, 22, 24, 28, 32]) {
      expect(leadEnd(rem), `${rem}px root`).toBeGreaterThan(trailEnd(rem));
      expect(computeMinDrawerWidth(rem, LIGHTS_RESERVE_PX), `${rem}px root`)
        .toBe(Math.ceil(2 * leadEnd(rem)));
    }
    expect(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX)).toBe(360);
  });

  it('scales with the root font size, because the row does', () => {
    expect(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX))
      .toBeGreaterThan(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX));
  });

  it('holds the web row to the packaged build\'s floor, as title room it gains', () => {
    const webRow = Math.ceil(2 * Math.max(leadEnd(16, PAD_REM * 16), trailEnd(16)));
    expect(webRow).toBe(176);
    expect(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX) - webRow)
      .toBe(2 * (leadEnd(16) - trailEnd(16)));
  });

  it('never stops the drawer narrower than the web row needs, at any root', () => {
    for (const rem of [12, 16, 24, 32, 48, 200]) {
      const webRow = Math.ceil(2 * Math.max(leadEnd(rem, PAD_REM * rem), trailEnd(rem)));
      expect(computeMinDrawerWidth(rem, LIGHTS_RESERVE_PX), `${rem}px root`)
        .toBeGreaterThanOrEqual(webRow);
    }
  });

  it('exceeds the retired 260px constant exactly where the bug was reported', () => {
    expect(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX)).toBeGreaterThan(260);
  });

  it('holds the reserve fixed while the rem part scales', () => {
    const grew = computeMinDrawerWidth(24, LIGHTS_RESERVE_PX)
      - computeMinDrawerWidth(16, LIGHTS_RESERVE_PX);
    expect(grew).toBe(4 * BUTTON_REM * 8);
  });
});

describe('minDrawerWidth reads the live root', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('reads the reserve from CSS', () => {
    vi.stubGlobal('getComputedStyle', () => ({
      fontSize: '16px',
      getPropertyValue: (name: string) => (name === '--titlebar-lights-reserve' ? '90px' : ''),
    }));
    expect(minDrawerWidth()).toBe(computeMinDrawerWidth(16, 90));
  });

  it('falls back to the px literal when the property is unreadable', () => {
    expect(minDrawerWidth()).toBe(computeMinDrawerWidth(16, LIGHTS_RESERVE_PX));
  });

  it('tracks the live root font size', () => {
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '20px', getPropertyValue: () => '' }));
    expect(minDrawerWidth()).toBe(computeMinDrawerWidth(20, LIGHTS_RESERVE_PX));
  });

  it('answers the same floor whether or not the build stamps the overlay', () => {
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
    expect(minThreadPanePx()).toBe(338);
    expect(minContentPanePx()).toBe(360);
  });

  it('scale with the root, which is the whole point of deriving them', () => {
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '20px', getPropertyValue: () => '' }));
    expect(minThreadPanePx()).toBe(382);
    expect(minContentPanePx()).toBe(450);
  });

  it('stop fitting a 1280px screen from 150% ui-scale, which the clamp must handle', () => {
    const sumAt = (fontSize: string) => {
      vi.stubGlobal('getComputedStyle', () => ({ fontSize, getPropertyValue: () => '' }));
      return minDrawerWidth() + minThreadPanePx() + minContentPanePx();
    };
    expect(sumAt('22px')).toBeLessThanOrEqual(1280);
    expect(sumAt('24px')).toBeGreaterThan(1280);
  });
});

describe('the TS mirror of the row still matches the CSS', () => {
  const rules = cssRules(shellCss);
  const DESKTOP = '@media (--desktop-layout)';
  const desktopRoot = rules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
  const paneHeader = rules.find(r => r.selector === '.pane-header');

  const row = rules.find(r => r.selector === '.threads-header' && r.atRules === DESKTOP);

  it('the icon box is 2.25rem: the toggle, Filter, Grouping and Search', () => {
    expect(desktopRoot?.props.get('--header-icon-box')).toBe('2.25rem');
  });

  it('the row gap is 0.25rem, one per button', () => {
    expect(paneHeader?.props.get('--pane-header-gap')).toBe('0.25rem');
  });

  it('the row pads 0.5rem at its trailing end and the toggle\'s room at its leading one', () => {
    expect(row?.props.get('padding')).toBe(`0 ${PAD_REM}rem 0 var(--threads-row-lead)`);
    expect(row?.props.get('--threads-row-lead'))
      .toBe('calc(var(--header-lead-inset) + var(--header-icon-box) + var(--pane-header-gap))');
  });

  it('the lights reserve still SUMS to the px value the fallback restates', () => {
    const reserve = desktopRoot!.props.get('--titlebar-lights-reserve')!;
    const x = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    expect(x, 'the reserve no longer derives from a stamped x').toBeGreaterThan(0);
    const cluster = parseInt(desktopRoot!.props.get('--titlebar-lights-cluster')!, 10);
    const gap = parseInt(desktopRoot!.props.get('--titlebar-lights-gap')!, 10);
    expect(x + cluster + gap).toBe(LIGHTS_RESERVE_PX);
  });

  it('the overlay build leads with exactly the reserve the floor assumes', () => {
    const overlayRoot = rules.find(
      r => r.selector === ':root[data-titlebar-overlay]' && r.atRules === DESKTOP,
    );
    expect(overlayRoot?.props.get('--header-lead-inset')).toBe('var(--titlebar-lights-reserve)');
  });

  it('the centred title clamps to the same two ends the floor pays twice', () => {
    const title = rules.find(
      r => r.selector === '.threads-header-title' && r.atRules === DESKTOP,
    );
    const clamp = title?.props.get('max-width') ?? '';
    expect(clamp).toContain('2 * max(var(--threads-row-lead) + var(--header-icon-box) + var(--pane-header-gap),');
    expect(clamp).toContain(`${PAD_REM}rem + 2 * (var(--header-icon-box) + var(--pane-header-gap))`);
  });
});
