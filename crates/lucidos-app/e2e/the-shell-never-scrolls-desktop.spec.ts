import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

/**
 * The desktop shell never scrolls as a whole.
 *
 * Reported from the packaged macOS app: the shell slid up its window, header
 * first, until only the page background showed. Scrolling or a reopen put it
 * back. The traffic lights followed the shrinking band, so the page saw the
 * shift itself, and on desktop only two boxes can carry it: the document and
 * `.app-shell`. Both are meant to hold still, and neither had a guard.
 *
 * Nothing in the shell makes either box overflow today, so the spec makes them
 * overflow on purpose. A 100vw box under a classic scrollbar, or an escaped
 * absolute box, is all it takes. It then asserts what the fix promises:
 * `styles/global/base.css` locks the root against wheel and rubber-band, and
 * `utils/shellScrollGuard.ts` snaps a scripted offset back.
 *
 * The overlay build is stamped, not run: no WebDriver reaches WKWebView (ADR
 * 0016). It matters here because the band is what the lights centre on.
 */

/** What `titlebar_inset_script` stamps. */
const TITLEBAR_INSET_PX = 28;

async function stampOverlayBuild(page: Page): Promise<void> {
  await page.evaluate((inset) => {
    document.documentElement.style.setProperty('--titlebar-inset', `${inset}px`);
    document.documentElement.setAttribute('data-titlebar-overlay', '');
  }, TITLEBAR_INSET_PX);
}

/** Both boxes' offsets, and the band's painted bottom edge. */
async function readShell(page: Page) {
  return page.evaluate(() => {
    const root = document.scrollingElement!;
    const shell = document.querySelector<HTMLElement>('.app-shell')!;
    const band = document.querySelector<HTMLElement>('[data-titlebar-band]')!;
    return {
      document: [root.scrollLeft, root.scrollTop],
      shell: [shell.scrollLeft, shell.scrollTop],
      bandBottom: band.getBoundingClientRect().bottom,
    };
  });
}

/** Give a box far more content than it shows, so it has room to scroll. */
async function overflow(page: Page, selector: string): Promise<void> {
  await page.evaluate((sel) => {
    const spacer = document.createElement('div');
    spacer.dataset.e2eSpacer = '';
    spacer.style.cssText = 'flex: none; width: 300vw; height: 300vh;';
    document.querySelector(sel)!.appendChild(spacer);
  }, selector);
}

test.describe('the desktop shell never scrolls as a whole', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await stampOverlayBuild(page);
  });

  test('the root is locked against wheel scroll and rubber-band', async ({ page }) => {
    const root = await page.evaluate(() => {
      const cs = getComputedStyle(document.documentElement);
      return { overflow: cs.overflow, overscroll: cs.overscrollBehaviorY };
    });
    expect(root).toEqual({ overflow: 'hidden', overscroll: 'none' });

    await overflow(page, 'body');
    // Over whichever pane holds the middle of the window. Each one ends its
    // own scroll, so the rest of the wheel chains on towards the root.
    const vp = page.viewportSize()!;
    await page.mouse.move(vp.width / 2, vp.height / 2);
    for (let i = 0; i < 8; i++) await page.mouse.wheel(600, 600);
    await page.waitForTimeout(300);
    expect((await readShell(page)).document, 'a wheel past the pane never reaches the root').toEqual([0, 0]);
  });

  test('a scripted offset on either box snaps back, and the band stays where the lights centre', async ({ page }) => {
    const before = await readShell(page);
    expect(before.bandBottom).toBeGreaterThan(TITLEBAR_INSET_PX);

    await overflow(page, 'body');
    await page.evaluate(() => window.scrollTo(400, 600));
    await expect.poll(async () => (await readShell(page)).document).toEqual([0, 0]);

    await overflow(page, '.app-shell');
    await page.evaluate(() => {
      const shell = document.querySelector<HTMLElement>('.app-shell')!;
      shell.scrollTop = 300;
      shell.scrollLeft = 200;
    });
    await expect.poll(async () => (await readShell(page)).shell).toEqual([0, 0]);

    expect((await readShell(page)).bandBottom, 'the header band is back at its own height').toBeCloseTo(before.bandBottom, 1);
  });
});
