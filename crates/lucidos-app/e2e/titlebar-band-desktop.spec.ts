import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

/**
 * The bar the macOS traffic lights are centred on is the one on screen.
 *
 * The shell places the OS buttons itself (`src/traffic_lights.rs`) against a
 * height only the page can measure, and `measureHeaderBarHeight` is that
 * measurement: the bottom edge of `[data-titlebar-band]`. Nothing else in the
 * suite resolves it, because the value is pure layout. The unit tests stub the
 * rect, and a CSS scan cannot add `--titlebar-inset` to a rem.
 *
 * The bug this pins. A packaged window at 125% ui-scale centred its lights 17pt
 * down a 60pt bar. That is where AppKit's own default sits, and where a 32px bar
 * would put them. On this build 32px is the header alone, without the reclaimed
 * strip. So the band being the sum of both parts is worth asserting at every
 * scale.
 *
 * The build is stamped rather than run: no WebDriver reaches WKWebView (ADR
 * 0016), so both facts `titlebar_inset_script` stamps pre-paint are set here by
 * hand. Unlike the sibling clearance spec, `--titlebar-inset` is the point
 * rather than an irrelevance, since it is the half of the bar the strip paints.
 */

test.use({ viewport: { width: 1280, height: 800 } });

/** Every ui-scale the sweep covers. 75 is UI_SCALE_MIN and 150 is where the
 *  fixed 28px strip is the smallest share of a rem-sized bar. */
const SCALES = [75, 100, 125, 150];

/** What `titlebar_inset_script` stamps. Fixed px, because it is the height of OS
 *  chrome and does not move with the user's scale. */
const TITLEBAR_INSET_PX = 28;

/** Subpixel tolerance: a rem-sized bar at 75% lands on a fraction. */
const EPS = 0.6;

interface Band {
  /** The live root font size, since the bar is `3rem` off it. */
  rootFontPx: number;
  /** What `measureHeaderBarHeight` reads, and therefore what the shell is told. */
  measured: number;
  /** The reclaimed strip, which paints the part of the bar above the header. */
  strip: number;
  /** The header's own box, which is the bar less the strip on this build. */
  header: number;
  /** The guard `measureHeaderBarHeight` applies before trusting the rect. */
  passesTransformGuard: boolean;
}

async function readBand(page: Page): Promise<Band | null> {
  return page.evaluate(() => {
    const band = document.querySelector('[data-titlebar-band]') as HTMLElement | null;
    const strip = document.querySelector('.titlebar-strip');
    if (!band || !strip) return null;
    const rect = band.getBoundingClientRect();
    if (rect.height === 0) return null;
    return {
      rootFontPx: parseFloat(getComputedStyle(document.documentElement).fontSize),
      measured: rect.bottom,
      strip: strip.getBoundingClientRect().height,
      header: rect.height,
      passesTransformGuard: rect.bottom >= band.offsetHeight - 1,
    };
  });
}

/** Stamp both facts the packaged build carries, the way the shell does. */
async function stampOverlayBuild(page: Page): Promise<void> {
  await page.evaluate((inset) => {
    document.documentElement.style.setProperty('--titlebar-inset', `${inset}px`);
    document.documentElement.setAttribute('data-titlebar-overlay', '');
  }, TITLEBAR_INSET_PX);
}

/** Set the scale and wait for the band to lay out at it. The row is rem-sized,
 *  so a scale write relays it out, and `loadPreferences` can re-apply the
 *  account's own scale over this one. */
async function settleAt(page: Page, scale: number): Promise<Band> {
  await page.evaluate(
    (s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`),
    scale,
  );
  let last: Band | null = null;
  await expect
    .poll(async () => {
      last = await readBand(page);
      return !!last && Math.abs(last.rootFontPx - 16 * scale / 100) < 0.1;
    }, { timeout: 10_000, message: `the band never laid out at ui-scale ${scale}` })
    .toBe(true);
  return last!;
}

/** `loadPreferences` ends in `applyUiScale`, which persists the scale. Until it
 *  has, a scale this sweep writes is one write away from being replaced. */
async function waitForScaleApplied(page: Page): Promise<void> {
  await page.waitForFunction(
    () => localStorage.getItem('lucidos-ui-scale') !== null,
    undefined,
    { timeout: 10_000 },
  );
}

test.describe('the titlebar band the lights centre on', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForScaleApplied(page);
  });

  test('packaged macOS: the band is the strip plus the header, at every scale', async ({ page }) => {
    await stampOverlayBuild(page);
    for (const scale of SCALES) {
      const band = await settleAt(page, scale);
      const where = `ui-scale ${scale}`;
      expect(band.strip, `${where}: the strip paints the reclaimed band`)
        .toBeCloseTo(TITLEBAR_INSET_PX, 1);
      expect(
        band.measured,
        `${where}: the band measures ${band.measured.toFixed(1)}, `
          + `against a ${band.strip.toFixed(1)} strip and a ${band.header.toFixed(1)} header`,
      ).toBeCloseTo(band.strip + band.header, 1);
      // The one number the whole arrangement is built from. This build SHORTENS
      // the header by the strip. Measuring the header alone would read 32px at
      // 125%, and centre the lights where AppKit already had them.
      expect(band.measured, `${where}: the bar is 3rem, whatever the parts`)
        .toBeCloseTo(3 * band.rootFontPx, 1);
      expect(band.header, `${where}: the header alone is NOT the bar`)
        .toBeLessThan(band.measured - EPS);
      expect(band.passesTransformGuard, `${where}: an at-rest header is not read as translated`)
        .toBe(true);
    }
  });

  test('web: the header IS the bar, since there is no band to reclaim', async ({ page }) => {
    // The control. Off the packaged build `--titlebar-inset` is unset and the
    // strip collapses to nothing. The same read has to stay right, rather than
    // subtracting a band that is not there.
    for (const scale of SCALES) {
      const band = await settleAt(page, scale);
      const where = `web, ui-scale ${scale}`;
      expect(band.strip, `${where}: no strip`).toBeCloseTo(0, 1);
      expect(band.measured, `${where}: the bar is 3rem here too`)
        .toBeCloseTo(3 * band.rootFontPx, 1);
      expect(band.measured, `${where}: and the header is all of it`)
        .toBeCloseTo(band.header, 1);
    }
  });
});
