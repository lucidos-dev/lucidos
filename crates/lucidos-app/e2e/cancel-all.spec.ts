import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage,
  assertHealthy, newThread, cancelStreamingResponse, countVisibleResponses,
  waitForStreamingToStart, waitForVisibleResponseCount, isMobileViewport,
} from './helpers';

test.describe('Cancel streaming response', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('cancel a streaming response via stop button', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg = uniqueMessage('cancel-stream');
    await sendMessage(page, `Write an extremely long and detailed essay about the entire history of mathematics from ancient times to modern day. Be as verbose as possible. Include: ${msg}`);

    await waitForStreamingToStart(page, 5, 60_000);

    await cancelStreamingResponse(page);

    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(1);
  });

  test('can send new message after canceling', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg1 = uniqueMessage('cancel-then-send');
    await sendMessage(page, `Write a very long essay about space exploration. Be extremely verbose. Include: ${msg1}`);

    await waitForStreamingToStart(page, 5, 60_000);

    await cancelStreamingResponse(page);

    const msg2 = uniqueMessage('after-cancel');
    await sendFollowUp(page, `Say exactly: "recovered ${msg2}"`);

    await waitForVisibleResponseCount(page, 2);

    await expect
      .poll(() => countVisibleResponses(page), { timeout: 30_000 })
      .toBeGreaterThanOrEqual(2);
  });

  test('a touch with no click still cancels', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'Touch behavior only, so the desktop project skips it');
    await navigateToApp(page);
    await newThread(page);

    const msg = uniqueMessage('cancel-by-touch');
    await sendMessage(page, `Write an extremely long and detailed essay about the history of cartography. Be as verbose as possible. Include: ${msg}`);
    await waitForStreamingToStart(page, 5, 60_000);

    const cancel = 'button.send-cancel-morph[aria-label="Cancel"]:not(:disabled)';
    await page.waitForSelector(cancel, { state: 'visible', timeout: 30_000 });
    await page.evaluate((sel) => {
      const el = Array.from(document.querySelectorAll<HTMLElement>(sel)).find(
        (c) => c.getBoundingClientRect().width > 0,
      );
      if (!el) throw new Error(`no visible element for ${sel}`);
      const r = el.getBoundingClientRect();
      const x = r.left + r.width / 2;
      const y = r.top + r.height / 2;

      const down = new Event('pointerdown', { bubbles: true, cancelable: true, composed: true });
      Object.defineProperty(down, 'screenX', { value: x });
      Object.defineProperty(down, 'screenY', { value: y });
      el.dispatchEvent(down);

      const point = { identifier: 1, target: el, clientX: x, clientY: y, screenX: x, screenY: y };
      const start = new Event('touchstart', { bubbles: true, cancelable: true, composed: true });
      Object.defineProperty(start, 'touches', { value: [point] });
      Object.defineProperty(start, 'targetTouches', { value: [point] });
      Object.defineProperty(start, 'changedTouches', { value: [point] });
      el.dispatchEvent(start);

      const up = new Event('touchend', { bubbles: true, cancelable: true, composed: true });
      Object.defineProperty(up, 'touches', { value: [] });
      Object.defineProperty(up, 'targetTouches', { value: [] });
      Object.defineProperty(up, 'changedTouches', { value: [point] });
      el.dispatchEvent(up);
    }, cancel);

    await page.waitForFunction(() => {
      const labels = document.querySelectorAll('.exchange-status-label');
      return Array.from(labels).some((el) => {
        const rect = el.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) return false;
        return (el.textContent ?? '').includes('Canceled');
      });
    }, undefined, { timeout: 30_000 });
  });
});
