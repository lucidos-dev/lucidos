import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, isMobileViewport } from './helpers';

/** Opening a thread lands the reader on the newest turn, and KEEPS them there
 *  while the transcript settles.
 *
 *  Reported as "scrolling to end when opening thread sometimes doesnt work",
 *  on desktop Chrome and intermittently. The pin a thread open arms used to be
 *  time-boxed at 500ms, and any growth landing after that window closed was
 *  read by the transcript's ResizeObserver as the reader having scrolled up.
 *  Every auto-scroll path then deferred to that, so the reader was stranded
 *  above the last turn with the down chevron on and nothing to recover it.
 *
 *  A markdown image is enough to do it in the wild (no reserved box, so it adds
 *  up to 24rem whenever it happens to decode late). This test grows the last
 *  turn from the page instead, well after the window, because that reproduces
 *  the mechanism deterministically: a resize with NO accompanying render, which
 *  is exactly the case no layout effect covers.
 *
 *  Desktop only. The same rule runs on mobile, but the mobile header's own
 *  scroll compensation is a second writer over the same offset and the
 *  assertion would be about it rather than about the resize rule. */
test.describe('Opening a thread lands at the end (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a late grow keeps the transcript on the newest turn', async ({ page }) => {
    test.skip(isMobileViewport(page), 'covered on desktop; mobile adds a second scroll writer');

    // A short viewport so a modest transcript overflows: with no scroll capacity
    // the assertion is vacuous.
    await page.setViewportSize({ width: 1280, height: 400 });
    await navigateToApp(page);

    await sendMessage(page, 'List the numbers from 1 to 40, one per line, and nothing else.');
    await waitForResponse(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    await expect.poll(() => tc.evaluate(el => el.scrollHeight - el.clientHeight)).toBeGreaterThan(0);
    await expect
      .poll(() => tc.evaluate(el => el.scrollTop + el.clientHeight >= el.scrollHeight - 2))
      .toBe(true);

    // Past the pin window (500ms) with room to spare, so the grow below lands in
    // the world the bug lived in: no suppression left, no render, no gesture.
    await page.waitForTimeout(1500);

    await tc.evaluate((el) => {
      const turns = el.querySelectorAll('.chat-exchange');
      const last = turns[turns.length - 1] as HTMLElement;
      const grown = document.createElement('div');
      grown.style.height = '600px';
      last.appendChild(grown);
    });

    // Still on the newest turn: the resize followed the bottom rather than
    // concluding the reader had scrolled away from it.
    await expect
      .poll(() => tc.evaluate(el => el.scrollTop + el.clientHeight >= el.scrollHeight - 2))
      .toBe(true);
    // And the chevron stays off, because there is nothing below to go to.
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);
  });
});
