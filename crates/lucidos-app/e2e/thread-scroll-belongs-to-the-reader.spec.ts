import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, isMobileViewport } from './helpers';

/** The transcript's scroll position belongs to the reader: the app moves it only
 *  when the reader asks, and two of those asks are STANDING rather than one-shot.
 *  Sending a message and pressing the down chevron each arm a follow that rides
 *  the live edge until the reader scrolls away.
 *
 *  A send arms it only when there is somewhere to be taken, which is why this
 *  spec opens on a BRAND-NEW thread and asserts the opposite there. The first
 *  message is the whole of what is on screen, so the reader can see it without
 *  moving and the send asks for nothing; riding would only drag them down
 *  through a first reply they have not read, which is what it used to do.
 *
 *  This file was `thread-open-lands-at-end-desktop.spec.ts` and asserted a
 *  time-boxed pin: a reply landed the reader on the newest turn, and a late grow
 *  KEPT them there for 500ms. The intermittent report behind it ("scrolling to
 *  end when opening thread sometimes doesnt work") came from a growth arriving
 *  after that window, which the ResizeObserver then read as the reader having
 *  scrolled up. Both the pin and the inference are gone. What replaced them is
 *  the flag this spec walks: armed by the reader, honoured for as long as they
 *  leave it armed, and retired the moment they scroll.
 *
 *  Growing the last turn from the page (rather than waiting for a markdown image
 *  to decode late) reproduces the mechanism deterministically: a resize with NO
 *  accompanying render, which is exactly the case no layout effect covers, and
 *  where both a stale re-pin and a follow that failed to follow would show.
 *
 *  Desktop only. The same rule runs on mobile, but the mobile header's own
 *  scroll compensation is a second writer over the same offset and the
 *  assertion would be about it rather than about the resize rule. */
test.describe('Transcript scroll belongs to the reader (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a new thread rides nothing, a later send rides the live edge, and a scroll up retires it', async ({ page }) => {
    test.skip(isMobileViewport(page), 'covered on desktop; mobile adds a second scroll writer');

    // A short viewport so a modest transcript overflows: with no scroll capacity
    // the assertions are vacuous.
    await page.setViewportSize({ width: 1280, height: 400 });
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const atBottom = () => tc.evaluate(el => el.scrollTop + el.clientHeight >= el.scrollHeight - 2);
    const growLastTurn = () => tc.evaluate((el) => {
      const turns = el.querySelectorAll('.chat-exchange');
      const last = turns[turns.length - 1] as HTMLElement;
      const grown = document.createElement('div');
      grown.style.height = '600px';
      last.appendChild(grown);
    });

    // The first message in a brand-new thread. It IS the thread, so there is
    // nothing to catch up to and nothing is armed: the 40-line reply grows past
    // the fold BELOW the reader, who has not moved a pixel and is offered the
    // chevron. This used to end on the answer's last line.
    await sendMessage(page, 'List the numbers from 1 to 40, one per line, and nothing else.');
    await waitForResponse(page);
    await expect.poll(() => tc.evaluate(el => el.scrollHeight - el.clientHeight)).toBeGreaterThan(100);
    await expect.poll(atBottom).toBe(false);
    // Still where they started, not merely short of the bottom. The slack
    // absorbs a repaint nudge's deliberate 1px, nothing near a drag.
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBeLessThan(50);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(1);

    // A send into that same thread DOES ask: the reader is above the fold now,
    // and what they write lands below it. So this one arms the follow, the
    // landing glide takes them to their own message, and the reply that streams
    // in below then carries them the rest of the way, leaving nothing for the
    // chevron to offer.
    await sendMessage(page, 'List the numbers from 1 to 20, one per line, and nothing else.');
    await waitForResponse(page);
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);

    // A real wheel gesture retires it. Everything after this point is the
    // unarmed reader, who owns their position absolutely.
    await tc.hover();
    await page.mouse.wheel(0, -600);
    await expect.poll(atBottom).toBe(false);
    const chevron = page.locator('button.scroll-to-bottom.visible');
    await expect(chevron).toHaveCount(1);

    // Past the old pin window (500ms) with room to spare, so the grow below lands
    // in the world the bug lived in: no suppression left, no render, no gesture.
    // It must not drag the reader after it.
    await page.waitForTimeout(1500);
    const before = await tc.evaluate(el => el.scrollTop);
    await growLastTurn();
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBe(before);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(1);

    // The chevron reaches the TRUE bottom of the grown content, and arms the
    // follow again on landing.
    await page.locator('button.scroll-to-bottom.visible').click();
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);

    // So this second grow, identical to the one above, carries them. Same
    // resize, opposite answer, and the only difference is that the reader asked.
    await growLastTurn();
    await expect.poll(atBottom).toBe(true);
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBeGreaterThan(before);
  });
});
