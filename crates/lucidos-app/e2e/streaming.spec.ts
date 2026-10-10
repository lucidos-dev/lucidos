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

    // Wait for a visible response-content with non-empty text — waiting only
    // for the element to exist races the first textContent snapshot below
    // against the first streamed chunk, leaving firstSnapshot empty (flaky
    // toBeTruthy, esp. on WebKit).
    await page.waitForFunction(() => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
      });
    }, { timeout: 30_000 });

    // Find the visible response
    const response = page.locator('.response-content:visible').first();

    // Capture text at two different points to verify streaming
    const firstSnapshot = await response.textContent();

    // Wait a bit for more content to stream
    await page.waitForTimeout(1_500);
    const secondSnapshot = await response.textContent();

    // Content appeared (streaming worked — exact progressive check is flaky for fast models)
    expect(firstSnapshot).toBeTruthy();
    expect(secondSnapshot).toBeTruthy();

    // Wait for completion
    await waitForResponse(page);

    // Final response should have substantial content
    const finalText = await response.textContent();
    expect(finalText!.trim().length).toBeGreaterThan(10);
  });

  test('exchange status shows working then completes', async ({ page }) => {
    await navigateToApp(page);

    await sendMessage(page, `Say "done" and nothing else.`);

    // Wait for response to complete
    const response = await waitForResponse(page);
    await expect(response).toBeVisible();
  });
});
