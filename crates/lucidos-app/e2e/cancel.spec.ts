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

    // Wait for streaming to start so there's content before canceling
    await waitForStreamingToStart(page, 5, 60_000);

    await cancelStreamingResponse(page);

    // Response should have partial content (not empty — text was streaming)
    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(1);
  });

  test('can send new message after canceling', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg1 = uniqueMessage('cancel-then-send');
    await sendMessage(page, `Write a very long essay about space exploration. Be extremely verbose. Include: ${msg1}`);

    // Wait for streaming to start before canceling
    await waitForStreamingToStart(page, 5, 60_000);

    await cancelStreamingResponse(page);

    // Send a new message and verify it works
    const msg2 = uniqueMessage('after-cancel');
    await sendFollowUp(page, `Say exactly: "recovered ${msg2}"`);

    // After a cancel the only status label is the settled "Canceled" one, so a
    // bare waitForResponse() can return before the follow-up turn even starts
    // streaming — then the count below sees just the canceled partial and
    // fails. Wait for the end-state instead: two visible responses with content
    // (the canceled partial + the follow-up's reply).
    await waitForVisibleResponseCount(page, 2);

    // Poll rather than read once: the post-cancel turn's text can still be
    // rendering when waitForResponse returns (its "no Working/Requesting label"
    // check can pass in the brief window before the new turn's status label
    // mounts), so a single read races the second response into the DOM.
    await expect
      .poll(() => countVisibleResponses(page), { timeout: 30_000 })
      .toBeGreaterThanOrEqual(2);
  });

  // The reported bug, reproduced by dispatching the touch WITHOUT its click.
  // That is what iOS does when the keyboard dismisses under the finger. Cancel
  // had no touch path to fall back on, so the button was simply dead. The
  // probe logged it as `Cancel: dead` with the finger still, the node
  // connected and the row unchanged. `page.touchscreen.tap` would prove
  // nothing here: it sends the click too, which is the path that already
  // worked. See `docs/plans/2026-08-28-cancel-survives-the-ios-keyboard.md`.
  test('a touch with no click still cancels', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'Touch behavior only, so the desktop project skips it');
    await navigateToApp(page);
    await newThread(page);

    const msg = uniqueMessage('cancel-by-touch');
    await sendMessage(page, `Write an extremely long and detailed essay about the history of cartography. Be as verbose as possible. Include: ${msg}`);
    await waitForStreamingToStart(page, 5, 60_000);

    const cancel = 'button.send-cancel-morph[aria-label="Cancel"]:not(:disabled)';
    await page.waitForSelector(cancel, { state: 'visible', timeout: 30_000 });
    // The settle window holds the morphed Stop disabled for a moment after the
    // send. Waiting for `:not(:disabled)` above is what clears it.
    //
    // Plain `Event`s with hand-defined properties, not `page.dispatchEvent`
    // and not `new TouchEvent()`: that constructor is illegal in WebKit, the
    // engine this case exists for. Same shape as `sdk-iframe-tooltip.spec.ts`.
    //
    // A stationary `pointerdown` first, so the tap gate holds a real press and
    // the lift is ruled a tap rather than waved through as press-less. That is
    // what a finger does, and it also proves no earlier gesture left the gate
    // holding an abort.
    await page.evaluate((sel) => {
      // The visible one: a mobile layout keeps every pane mounted at once.
      const el = Array.from(document.querySelectorAll<HTMLElement>(sel)).find(
        (c) => c.getBoundingClientRect().width > 0,
      );
      if (!el) throw new Error(`no visible element for ${sel}`);
      const r = el.getBoundingClientRect();
      const x = r.left + r.width / 2;
      const y = r.top + r.height / 2;

      const down = new Event('pointerdown', { bubbles: true, cancelable: true, composed: true });
      // The gate reads SCREEN coordinates, and nothing else off the event.
      Object.defineProperty(down, 'screenX', { value: x });
      Object.defineProperty(down, 'screenY', { value: y });
      el.dispatchEvent(down);

      const point = { identifier: 1, target: el, clientX: x, clientY: y };
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
