import { test, expect } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { gotoWithRetry } from './helpers';

const APP_ID = 'e2e-sdk-confirm-test';
const APP_DIR = resolve(WORKSPACE, 'data/apps', APP_ID);

test.describe('SDK lucidos.ui.confirm — host renders modal, returns Promise<boolean>', () => {
  test.beforeAll(() => {
    mkdirSync(APP_DIR, { recursive: true });
    writeFileSync(resolve(APP_DIR, 'index.html'), `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>SDK confirm test</title>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<button id="ask">Ask</button>
<div id="result">none</div>
<script>
  // Expose a helper so the test can trigger calls with any options.
  window.runConfirm = async function(opts) {
    const ok = await lucidos.ui.confirm(opts);
    document.getElementById('result').textContent = ok ? 'yes' : 'no';
    return ok;
  };
</script>
</body>
</html>
`);
    writeFileSync(resolve(APP_DIR, 'manifest.json'), JSON.stringify({
      id: APP_ID,
      name: 'SDK confirm test',
      description: 'e2e fixture',
    }));
  });

  test.afterAll(() => {
    rmSync(APP_DIR, { recursive: true, force: true });
  });

  async function setupIframe(page: import('@playwright/test').Page) {
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await gotoWithRetry(page, '/');
    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10000 });
    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ask')).toBeVisible({ timeout: 10000 });
    const handle = await iframeLoc.elementHandle();
    if (!handle) throw new Error('iframe handle missing');
    const frame = await handle.contentFrame();
    if (!frame) throw new Error('iframe contentFrame missing');
    return { loc: appFrame, frame };
  }

  test('OK click → resolves true; modal renders in host with title and labels', async ({ page }) => {
    const { loc, frame } = await setupIframe(page);

    const resultPromise = frame.evaluate(() =>
      (window as unknown as { runConfirm: (o: unknown) => Promise<boolean> }).runConfirm({
        title: 'Delete node?',
        message: 'Delete "Reduce CPAC by 50%" and its 3 descendants?',
        okLabel: 'Delete',
        cancelLabel: 'Keep',
        danger: true,
      })
    );

    await expect(page.locator('.confirm-dialog')).toBeVisible();
    await expect(page.locator('.confirm-dialog .surface-title')).toHaveText('Delete node?');
    await expect(page.locator('.confirm-message')).toContainText('Reduce CPAC by 50%');
    await expect(page.locator('[data-role="confirm-ok"]')).toHaveText('Delete');
    await expect(page.locator('[data-role="confirm-ok"]')).toHaveClass(/\baction-btn-danger\b/);
    await expect(page.locator('[data-role="confirm-cancel"]').first()).toHaveText('Keep');

    await page.locator('[data-role="confirm-ok"]').click();
    expect(await resultPromise).toBe(true);
    await expect(loc.locator('#result')).toHaveText('yes');
  });

  test('Cancel click → resolves false', async ({ page }) => {
    const { frame } = await setupIframe(page);
    const resultPromise = frame.evaluate(() =>
      (window as unknown as { runConfirm: (o: unknown) => Promise<boolean> }).runConfirm({ message: 'Continue?' })
    );
    await expect(page.locator('.confirm-dialog')).toBeVisible();
    await page.locator('[data-role="confirm-cancel"]').first().click();
    expect(await resultPromise).toBe(false);
  });

  test('Esc → resolves false', async ({ page }) => {
    const { frame } = await setupIframe(page);
    const resultPromise = frame.evaluate(() =>
      (window as unknown as { runConfirm: (o: unknown) => Promise<boolean> }).runConfirm({ message: 'Continue?' })
    );
    await expect(page.locator('.confirm-dialog')).toBeVisible();
    await page.locator('.confirm-message').click();
    await page.keyboard.press('Escape');
    expect(await resultPromise).toBe(false);
  });

  test('Enter → resolves true', async ({ page }) => {
    const { frame } = await setupIframe(page);
    const resultPromise = frame.evaluate(() =>
      (window as unknown as { runConfirm: (o: unknown) => Promise<boolean> }).runConfirm({ message: 'Continue?', okLabel: 'Yes' })
    );
    await expect(page.locator('.confirm-dialog')).toBeVisible();
    await page.locator('.confirm-message').click();
    await page.keyboard.press('Enter');
    expect(await resultPromise).toBe(true);
  });

  test('Backdrop click → resolves false', async ({ page }) => {
    const { frame } = await setupIframe(page);
    const resultPromise = frame.evaluate(() =>
      (window as unknown as { runConfirm: (o: unknown) => Promise<boolean> }).runConfirm({ message: 'Continue?' })
    );
    await expect(page.locator('.confirm-dialog')).toBeVisible();
    await page.mouse.click(2, 2);
    expect(await resultPromise).toBe(false);
  });

  test('Second confirm replaces first; first resolves false', async ({ page }) => {
    const { frame } = await setupIframe(page);

    const first = frame.evaluate(() =>
      (window as unknown as { runConfirm: (o: unknown) => Promise<boolean> }).runConfirm({ message: 'First?' })
    );
    await expect(page.locator('.confirm-message')).toHaveText('First?');

    const second = frame.evaluate(() =>
      (window as unknown as { runConfirm: (o: unknown) => Promise<boolean> }).runConfirm({ message: 'Second?', okLabel: 'Go', danger: true })
    );
    await expect(page.locator('.confirm-message')).toHaveText('Second?');

    expect(await first).toBe(false);

    await page.locator('[data-role="confirm-ok"]').click();
    expect(await second).toBe(true);
  });

  test('danger: false renders default OK styling', async ({ page }) => {
    const { frame } = await setupIframe(page);
    const resultPromise = frame.evaluate(() =>
      (window as unknown as { runConfirm: (o: unknown) => Promise<boolean> }).runConfirm({ message: 'OK?', danger: false })
    );
    const ok = page.locator('[data-role="confirm-ok"]');
    await expect(ok).toBeVisible();
    await expect(ok).not.toHaveClass(/\baction-btn-danger\b/);
    await ok.click();
    expect(await resultPromise).toBe(true);
  });
});
