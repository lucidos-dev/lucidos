/**
 * An app page never scrolls sideways (ADR 0530).
 *
 * The served `/api/v1/sdk-iframe.css` clips `<html>` and `<body>` on the x
 * axis. The fixture carries the culprits that made real apps scroll: a
 * `flex: none` nowrap pill holding long text, a `100vw` block with a margin,
 * a negative margin and a wide image. A chip row is the one deliberate
 * sideways scroller, and it must still scroll inside itself.
 */
import type { Frame } from '@playwright/test';
import { test, expect, type Page } from './fixtures';
import { appPath, createIframeAppFixture } from './db-helpers';
import { assertHealthy, ensureMobileView, gotoWithRetry } from './helpers';

const APP_ID = 'e2e-no-horizontal-scroll';
const WIDTHS = [320, 375, 390, 430];
const FRAME = '[data-role="app-ui-panel"] iframe[data-role="app-ui-frame"]';

const HTML = `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<script src="/api/v1/sdk-prefs.js"></script>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<style>
  .row { display: flex; gap: 0.5rem; }
  .pill { flex: none; white-space: nowrap; }
  .bleed { width: 100vw; margin-left: 2rem; height: 1rem; }
  .pull { margin-right: -5rem; height: 1rem; }
  .chips { display: flex; gap: 0.5rem; overflow-x: auto; }
  .chips span { flex: none; white-space: nowrap; }
  .tall { height: 300vh; }
</style>
</head>
<body>
<div class="row"><span>Skipped</span><span class="pill">${'A long free-text note that never wraps '.repeat(8)}</span></div>
<div class="bleed"></div>
<div class="pull"></div>
<img width="1600" height="40" alt="" src="data:image/gif;base64,R0lGODlhAQABAAAAACw=">
<div class="chips">${Array.from({ length: 20 }, (_, i) => `<span>Chip number ${i}</span>`).join('')}</div>
<div class="tall"></div>
<div id="ready">ready</div>
</body>
</html>
`;

let fixture: { cleanup: () => void };

test.describe('An app page never scrolls sideways', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, { html: HTML, js: '' });
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  /** What the page reports after a sideways scroll attempt by code. */
  async function measure(target: Page | Frame) {
    // The served stylesheet loads after `#ready` parses on a slow first load.
    await target.waitForFunction(() => getComputedStyle(document.documentElement).overflowX === 'clip');
    return target.evaluate(() => {
      const page = document.scrollingElement!;
      window.scrollTo(500, 0);
      const chips = document.querySelector<HTMLElement>('.chips')!;
      chips.scrollLeft = 200;
      return {
        scrollWidth: page.scrollWidth,
        clientWidth: page.clientWidth,
        scrollX: window.scrollX,
        // Emulated mobile WebKit draws the page's classic scrollbar inside the
        // layout viewport, so code can pan by its width. An iPhone's scrollbar
        // is an overlay and takes no width.
        scrollbarWidth: window.innerWidth - window.visualViewport!.width,
        chipsScrollWidth: chips.scrollWidth,
        chipsClientWidth: chips.clientWidth,
        chipsScrollLeft: chips.scrollLeft,
        bodyOverflowY: getComputedStyle(document.body).overflowY,
      };
    });
  }

  for (const width of WIDTHS) {
    test(`at ${width}px the page fits and only the chip row scrolls`, async ({ page }) => {
      await page.setViewportSize({ width, height: 800 });
      await gotoWithRetry(page, appPath(APP_ID));
      await expect(page.locator('#ready')).toBeAttached();

      const m = await measure(page);
      expect(m.scrollWidth, 'the page is no wider than the viewport').toBeLessThanOrEqual(m.clientWidth);
      expect(m.scrollX, 'code cannot scroll the page sideways').toBeLessThanOrEqual(m.scrollbarWidth);
      expect(m.chipsScrollWidth).toBeGreaterThan(m.chipsClientWidth);
      expect(m.chipsScrollLeft, 'the chip row still scrolls inside itself').toBeGreaterThan(0);
      expect(m.bodyOverflowY, 'body is no scroll container').toBe('visible');

      await page.evaluate(() => window.scrollTo(0, 400));
      expect(await page.evaluate(() => window.scrollY), 'the page still scrolls down').toBeGreaterThan(0);
    });
  }

  test('in the host shell the app page does not scroll sideways', async ({ page }) => {
    await assertHealthy(page);
    await page.addInitScript((id) => localStorage.setItem('app-window-open', id), APP_ID);
    await gotoWithRetry(page, '/');
    await ensureMobileView(page, 'content');
    await expect(page.locator(FRAME)).toBeVisible({ timeout: 15_000 });

    const frame = (await (await page.locator(FRAME).elementHandle())!.contentFrame())!;
    await expect(frame.locator('#ready')).toBeAttached();
    const m = await measure(frame);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.clientWidth);
    expect(m.scrollX).toBeLessThanOrEqual(m.scrollbarWidth);
  });
});
