import { test, expect, type Page } from './fixtures';
import {
  apiRequest, assertHealthy, clickVisibleElement, ensureOnThreadPane, gotoWithRetry,
  openFilesPanel, waitForVisibleInput,
} from './helpers';

async function screenSize(page: Page): Promise<{ cssWidth: number; ratio: number }> {
  return page.evaluate(() => ({
    cssWidth: window.innerWidth,
    ratio: window.devicePixelRatio,
  }));
}

async function drawnAgainstNatural(page: Page): Promise<{ drawn: number; natural: number }> {
  return page.locator('.image-popup-slide img').first().evaluate((el) => {
    const img = el as HTMLImageElement;
    return {
      drawn: img.getBoundingClientRect().width * window.devicePixelRatio,
      natural: img.naturalWidth,
    };
  });
}

async function readoutPercent(page: Page): Promise<number> {
  return parseInt(await page.locator('.image-popup-zoom-level').innerText(), 10);
}

let created: string[] = [];

async function openImageOfWidth(page: Page, width: number): Promise<void> {
  const name = `e2e-popup-density-${width}-${Date.now()}.png`;
  const path = `artifacts/${name}`;
  const status = await page.evaluate(async ({ target, w }) => {
    const canvas = document.createElement('canvas');
    canvas.width = w;
    canvas.height = Math.round(w * 0.6);
    const ctx = canvas.getContext('2d');
    if (!ctx) return 0;
    ctx.fillStyle = '#123456';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    const blob = await new Promise<Blob | null>(done => canvas.toBlob(done, 'image/png'));
    if (!blob) return 0;
    const deviceId = localStorage.getItem('lucidos-device-id');
    const resp = await fetch(`/api/v1/data/${target}`, {
      method: 'PUT',
      headers: {
        'Content-Type': 'image/png',
        ...(deviceId ? { 'x-lucidos-device-id': deviceId } : {}),
      },
      body: await blob.arrayBuffer(),
    });
    if (resp.status !== 200) return resp.status;
    const stored = await fetch(`/data/${target}`, { cache: 'no-store' });
    return (await stored.arrayBuffer()).byteLength === blob.size ? resp.status : -1;
  }, { target: path, w: width });
  created.push(path);
  expect(status, 'the fixture upload').toBe(200);

  await openFilesPanel(page);
  await page.waitForFunction((n) =>
    Array.from(document.querySelectorAll('.file-item')).some((el) => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(n);
    }),
  name, { timeout: 15_000 });
  expect(await clickVisibleElement(page, '.file-item', name)).toBe(true);

  const preview = page.locator('.preview-image:visible').first();
  await expect(preview).toBeVisible({ timeout: 10_000 });
  await preview.click();
  await expect(page.locator('.image-popup-content')).toBeVisible({ timeout: 5_000 });

  await page.waitForFunction(() => {
    const img = document.querySelector('.image-popup-slide img') as HTMLImageElement | null;
    return !!img && img.complete && img.naturalWidth > 0 && img.style.transform !== '';
  }, undefined, { timeout: 10_000 });
  expect((await drawnAgainstNatural(page)).natural, 'the fixture arrived intact')
    .toBe(width);
}

test.describe('the image popup counts physical screen pixels', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await gotoWithRetry(page, '/');
    await page.waitForFunction(() =>
      document.querySelector('#app')?.childElementCount! > 0,
      undefined, { timeout: 30_000 },
    );
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);
  });

  test.afterEach(async ({ page }) => {
    for (const path of created) await apiRequest(page).delete(`/api/v1/data/${path}`);
    created = [];
  });

  test('a capture of this screen, opened on it, is already actual size', async ({ page }) => {
    const { cssWidth, ratio } = await screenSize(page);
    await openImageOfWidth(page, Math.round(cssWidth * ratio));

    await expect(page.locator('.image-popup-zoom-level')).toBeDisabled();
    await expect(page.locator('.image-popup-zoom-btn[aria-label="Zoom out"]')).toBeDisabled();
    await expect(page.locator('.image-popup-zoom-btn[aria-label="Zoom in"]')).toBeEnabled();

    expect(await readoutPercent(page)).toBe(100);
    const { drawn, natural } = await drawnAgainstNatural(page);
    expect(Math.abs(drawn - natural), 'one image pixel per screen pixel').toBeLessThan(2);

    await page.locator('.floating-mobile-close').click();
    await expect(page.locator('.image-popup-content')).toHaveCount(0);
  });

  test('twice this screen reads 50%, and actual size doubles it', async ({ page }) => {
    const { cssWidth, ratio } = await screenSize(page);
    await openImageOfWidth(page, Math.round(cssWidth * ratio * 2));

    await expect.poll(() => readoutPercent(page), { timeout: 5_000 }).toBe(50);

    const level = page.locator('.image-popup-zoom-level');
    await expect(level).toBeEnabled();
    await level.click();
    await expect.poll(() => readoutPercent(page), { timeout: 5_000 }).toBe(100);
    const { drawn, natural } = await drawnAgainstNatural(page);
    expect(Math.abs(drawn - natural), 'one image pixel per screen pixel').toBeLessThan(2);

    await level.click();
    await expect.poll(() => readoutPercent(page), { timeout: 5_000 }).toBe(50);
  });
});
