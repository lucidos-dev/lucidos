import { test, expect, type Page, type Frame } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import {
  assertHealthy, disableMobileDynamicBars, enableMobileDynamicBars, ensureMobileView, gotoWithRetry,
} from './helpers';

test.use({ reducedMotion: 'no-preference' });

const APP_ID = 'e2e-dynamic-bars-app';
const FRAME = '.mobile-swipe-pane [data-role="app-ui-panel"] iframe[data-role="app-ui-frame"]';
let fixture: { cleanup: () => void };

test.use({ viewport: { width: 393, height: 852 } });

interface Geometry {
  headerTop: number;
  headerBottom: number;
  frameTop: number;
  frameBottom: number;
  paneBottom: number;
  away: boolean;
}

async function geometry(page: Page): Promise<Geometry> {
  return page.evaluate((sel) => {
    const frame = document.querySelector(sel)!;
    const header = document.querySelector('.app-header')!.getBoundingClientRect();
    const f = frame.getBoundingClientRect();
    const pane = frame.closest('.content-pane-body')!.getBoundingClientRect();
    return {
      headerTop: header.top,
      headerBottom: header.bottom,
      frameTop: f.top,
      frameBottom: f.bottom,
      paneBottom: pane.bottom,
      away: frame.closest('[data-role="app-ui-panel"]')!.hasAttribute('data-bars-away'),
    };
  }, FRAME);
}

async function appFrame(page: Page): Promise<Frame> {
  await expect(page.locator(FRAME)).toBeVisible({ timeout: 15_000 });
  const handle = await page.locator(FRAME).elementHandle();
  const frame = (await handle!.contentFrame())!;
  await expect(frame.locator('.row').nth(199)).toBeAttached({ timeout: 10_000 });
  return frame;
}

async function scrollApp(frame: Frame, tops: number[]) {
  for (const top of tops) {
    await frame.evaluate((y) => new Promise<void>((done) => {
      window.scrollTo(0, y);
      requestAnimationFrame(() => requestAnimationFrame(() => done()));
    }), top);
  }
}

async function settled(page: Page): Promise<Geometry> {
  let last: Geometry | null = null;
  await expect.poll(async () => {
    const now = await geometry(page);
    const still = last !== null && JSON.stringify(now) === JSON.stringify(last);
    last = now;
    return still;
  }, { timeout: 5_000, intervals: [200] }).toBe(true);
  return last!;
}

test.describe('dynamic bars over an app', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Dynamic bars app', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>Dynamic bars app</title>
<script src="/api/v1/sdk.js"></script>
<style>body { margin: 0; } .row { height: 50px; border-bottom: 1px solid #ccc; }</style>
</head><body><script src="script.js"></script></body></html>`,
      js: `for (let i = 0; i < 200; i++) {
  const div = document.createElement('div');
  div.className = 'row';
  div.textContent = 'Row ' + i;
  document.body.appendChild(div);
}`,
    });
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await enableMobileDynamicBars(page);
    await page.addInitScript((id) => localStorage.setItem('app-window-open', id), APP_ID);
    await gotoWithRetry(page, '/');
    await ensureMobileView(page, 'content');
  });

  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  test('the header leaves on a scroll down and the app takes its room', async ({ page }) => {
    const frame = await appFrame(page);
    const shown = await settled(page);
    expect(shown.headerTop).toBe(0);
    expect(shown.frameTop).toBeCloseTo(shown.headerBottom, 0);

    await scrollApp(frame, [100, 300, 600]);
    const away = await settled(page);
    expect(away.away).toBe(true);
    expect(away.headerBottom).toBeLessThanOrEqual(0.5);
    expect(away.frameTop).toBeCloseTo(0, 0);
    expect(away.frameBottom).toBeCloseTo(away.paneBottom, 0);
    expect(await frame.evaluate(() => window.scrollY)).toBe(600);

    await scrollApp(frame, [560, 500]);
    const back = await settled(page);
    expect(back.away).toBe(false);
    expect(back.headerTop).toBe(0);
    expect(back.frameTop).toBeCloseTo(back.headerBottom, 0);
    expect(back.frameBottom).toBeCloseTo(back.paneBottom, 0);
  });

  test('the header stays near the top of the app', async ({ page }) => {
    const frame = await appFrame(page);
    await settled(page);
    await scrollApp(frame, [10, 20]);
    const near = await settled(page);
    expect(near.away).toBe(false);
    expect(near.headerTop).toBe(0);
  });
});
