import { test, expect } from './fixtures';
import {
  apiRequest,
  assertHealthy,
  cancelStreamingResponse,
  clickVisibleElement,
  newThread,
  waitForStreamingToStart,
  focusedThreadId,
  navigateToApp,
  openThreadDrawer,
  sendMessage,
  threadRowFor,
  uniqueMessage,
  waitForResponse,
} from './helpers';
import { clearAllThreads } from './db-helpers';

const REASON = 'Waiting on the vendor fix';

test.describe('Hold', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('a held thread shows the strip and lists On hold until Resume', async ({ page }) => {
    await navigateToApp(page);
    await sendMessage(page, `Say exactly: "${uniqueMessage('hold')}"`);
    await waitForResponse(page);
    const threadId = await focusedThreadId(page);

    const held = await apiRequest(page).post(`/api/v1/threads/${threadId}/hold`, { data: { reason: REASON } });
    expect(held.ok(), `hold -> ${held.status()}`).toBe(true);

    const strip = page.locator('[data-role="hold-strip"]:visible');
    await expect(strip.locator('.hold-strip-label')).toHaveText(`On hold: ${REASON}`, { timeout: 15_000 });

    await openThreadDrawer(page);
    const onHold = page.locator('.drawer-section:visible', {
      has: page.locator('.section-label', { hasText: /^On hold$/ }),
    });
    await expect(onHold.locator(threadRowFor(threadId))).toBeVisible({ timeout: 10_000 });

    await strip.getByRole('button', { name: 'Resume' }).click();
    await expect(strip).toHaveCount(0, { timeout: 15_000 });
    await expect(onHold).toHaveCount(0);
    await expect(page.locator(`${threadRowFor(threadId)}:visible`)).toBeVisible();
  });

  test('after a Stop, Archive is one button and Hold sits in the thread menu', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await sendMessage(page, `Write a very long essay about rivers. Be extremely verbose. Include: ${uniqueMessage('stop-hold')}`);
    await waitForStreamingToStart(page, 5, 60_000);
    await cancelStreamingResponse(page);

    await expect(page.locator('button[aria-label^="Archive thread"]:visible')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('.split-button[data-thread-action]:visible')).toHaveCount(0);

    const opened = await clickVisibleElement(page, '.thread-title-menu, .thread-row-focused [aria-label="More thread actions"]');
    expect(opened, 'a thread menu is on screen').toBe(true);
    await page.locator('.thread-overflow-menu [role="menuitem"]', { hasText: 'Hold…' }).click();

    const dialog = page.locator('.surface:visible', { has: page.locator('textarea, input') });
    await dialog.locator('textarea, input').first().fill(REASON);
    await dialog.getByRole('button', { name: /^Hold$/ }).click();

    const strip = page.locator('[data-role="hold-strip"]:visible');
    await expect(strip.locator('.hold-strip-label')).toHaveText(`On hold: ${REASON}`, { timeout: 15_000 });
  });
});
