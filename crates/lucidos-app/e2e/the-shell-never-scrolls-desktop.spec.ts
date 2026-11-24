import { test, expect, type Page } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';


const TITLEBAR_INSET_PX = 28;

async function stampOverlayBuild(page: Page): Promise<void> {
  await page.evaluate((inset) => {
    document.documentElement.style.setProperty('--titlebar-inset', `${inset}px`);
    document.documentElement.setAttribute('data-titlebar-overlay', '');
  }, TITLEBAR_INSET_PX);
}

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
