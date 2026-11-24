import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp, watchPreferenceReads } from './helpers';


test.use({ viewport: { width: 1280, height: 800 } });

const SCALES = [75, 100, 125, 150];

const TITLEBAR_INSET_PX = 28;

const EPS = 0.6;

interface Band {
  rootFontPx: number;
  measured: number;
  strip: number;
  header: number;
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

async function stampOverlayBuild(page: Page): Promise<void> {
  await page.evaluate((inset) => {
    document.documentElement.style.setProperty('--titlebar-inset', `${inset}px`);
    document.documentElement.setAttribute('data-titlebar-overlay', '');
  }, TITLEBAR_INSET_PX);
}

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

test.describe('the titlebar band the lights centre on', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    const preferencesSettled = watchPreferenceReads(page);
    await navigateToApp(page);
    await preferencesSettled();
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
      expect(band.measured, `${where}: the bar is 3rem, whatever the parts`)
        .toBeCloseTo(3 * band.rootFontPx, 1);
      expect(band.header, `${where}: the header alone is NOT the bar`)
        .toBeLessThan(band.measured - EPS);
      expect(band.passesTransformGuard, `${where}: an at-rest header is not read as translated`)
        .toBe(true);
    }
  });

  test('web: the header IS the bar, since there is no band to reclaim', async ({ page }) => {
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
