/**
 * An explainer opens at its info icon, on every client.
 *
 * It was a centred modal with a scrim. It now hangs off the icon like the Waits
 * panel, clamped inside the pane that holds it (the viewport on a phone). Its
 * head and body share the surface inset. So the title and the first paragraph
 * start on one edge, one root em in from the frame.
 *
 * Runs on all three projects: the phone is where a popover near the screen edge
 * would clip, which is the case the clamp exists for.
 */
import { test, expect, Page } from './fixtures';
import { apiRequest, assertHealthy, isMobileViewport, navigateToApp, waitForEventStream } from './helpers';

const INFO = 'button[aria-label="About Motion"]';

async function openAppearance(page: Page): Promise<void> {
  await navigateToApp(page);
  await waitForEventStream(page);
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'settings', params: { settings_view: 'appearance' } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
  await expect(page.locator(INFO)).toBeVisible({ timeout: 15_000 });
}

test.describe('Explainer opens at its icon', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('hangs off the icon, inside its pane, with no scrim', async ({ page }) => {
    await openAppearance(page);
    const info = page.locator(INFO);
    await info.click();

    const popover = page.locator('.explainer-popover');
    await expect(popover).toBeVisible();
    await expect(popover.locator('.surface-title')).toHaveText('Motion');
    await expect(page.locator('.modal-overlay')).toHaveCount(0);

    const icon = (await info.boundingBox())!;
    const panel = (await popover.boundingBox())!;
    const below = panel.y - (icon.y + icon.height);
    const above = icon.y - (panel.y + panel.height);
    expect(Math.min(Math.abs(below), Math.abs(above)), 'panel hangs off the icon').toBeLessThan(12);

    const viewport = page.viewportSize()!;
    const box = isMobileViewport(page)
      ? { x: 0, y: 0, width: viewport.width, height: viewport.height }
      : (await page.locator('.split-layout > .pane-content').boundingBox())!;
    expect(panel.x).toBeGreaterThanOrEqual(box.x);
    expect(panel.x + panel.width).toBeLessThanOrEqual(box.x + box.width + 0.5);
    expect(panel.y + panel.height).toBeLessThanOrEqual(viewport.height + 0.5);
  });

  test('puts the title and the copy on one edge, one root em from the frame', async ({ page }) => {
    await openAppearance(page);
    await page.locator(INFO).click();
    const popover = page.locator('.explainer-popover');
    await expect(popover).toBeVisible();

    const { frame, title, copy, rootEm } = await popover.evaluate((el) => {
      const firstText = (node: Element) => {
        const range = document.createRange();
        range.selectNodeContents(node);
        return range.getClientRects()[0].left;
      };
      const rect = el.getBoundingClientRect();
      const border = parseFloat(getComputedStyle(el).borderLeftWidth);
      return {
        frame: rect.left + border,
        title: firstText(el.querySelector('.surface-title')!),
        copy: firstText(el.querySelector('.explainer-body p')!),
        rootEm: parseFloat(getComputedStyle(document.documentElement).fontSize),
      };
    });
    expect(Math.abs(title - copy), 'title and copy share an edge').toBeLessThan(1);
    expect(Math.abs(title - frame - rootEm), 'the inset is 1rem').toBeLessThan(1);
  });
});
