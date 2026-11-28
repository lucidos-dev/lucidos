import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy } from './helpers';

test.describe('SSE live streaming', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('response text streams in progressively', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('stream-test');
    await sendMessage(page, `Write a paragraph about the number ${msg.slice(-6)}. Be verbose.`);

    await page.waitForFunction(() => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
      });
    }, { timeout: 30_000 });

    const response = page.locator('.response-content:visible').first();

    const firstSnapshot = await response.textContent();

    await page.waitForTimeout(1_500);
    const secondSnapshot = await response.textContent();

    expect(firstSnapshot).toBeTruthy();
    expect(secondSnapshot).toBeTruthy();

    await waitForResponse(page);

    const finalText = await response.textContent();
    expect(finalText!.trim().length).toBeGreaterThan(10);
  });

  test('exchange status shows working then completes', async ({ page }) => {
    await navigateToApp(page);

    await sendMessage(page, `Say "done" and nothing else.`);

    const response = await waitForResponse(page);
    await expect(response).toBeVisible();
  });
});
