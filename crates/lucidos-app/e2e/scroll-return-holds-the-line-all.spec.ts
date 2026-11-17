import { test, expect } from './fixtures';
import {
  assertHealthy, clickThreadRow, disarmFollowSeed, ensureOnThreadPane, hideDashboardRail,
  navigateToApp, newThread, openThreadDrawer, sendMessageCapturingThreadId, sendMessage,
  waitForResponse, waitForVisibleInput, waitForVisibleResponseCount,
} from './helpers';

/** Coming back to a thread puts the reader on the LINE they left, not only
 *  on its turn, and holds it while the transcript settles. The report: "when i
 *  go back to threads, it's almost always scrolled higher up than where i left
 *  it". Plan: docs/plans/2026-11-12-conversation-return-fixes.md (I8, I12).
 *
 *  The reader parks inside a reply, partway down the turn. On return, a box
 *  grows at the top of that same turn, above their line, as a late widget or
 *  image does. Holding the turn's top lets that growth push the line down the
 *  viewport. Holding the line keeps it within 2 px.
 *
 *  All three projects: WebKit has no native scroll anchoring, so this hold is
 *  the only correction a Safari or macOS app reader gets. */
test.describe('a return holds the line the reader left', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await hideDashboardRail(page);
  });

  test('the reply the reader was reading stays put while the turn above it grows', async ({ page }) => {
    // A short viewport, so four mock replies overflow it several times over.
    const width = page.viewportSize()?.width ?? 1280;
    await page.setViewportSize({ width, height: 420 });
    // A rider is recorded as the live edge, which is not the case under test.
    await disarmFollowSeed(page);
    await navigateToApp(page);

    const threadId = await sendMessageCapturingThreadId(page, 'First message for the return spec.');
    await waitForResponse(page);
    for (let n = 2; n <= 4; n++) {
      await sendMessage(page, `Message ${n} for the return spec.`);
      await waitForVisibleResponseCount(page, n);
      await waitForResponse(page);
    }

    const tc = page.locator('.thread-content.visible:visible').first();
    /** Where turn `index`'s reply sits, measured from the transcript's top. */
    const replyTop = (index: number) => tc.evaluate((el, i) => {
      const turn = el.querySelectorAll('.chat-exchange')[i];
      const reply = turn?.querySelector('.response-content');
      if (!reply) return null;
      return reply.getBoundingClientRect().top - el.getBoundingClientRect().top;
    }, index);

    // Park 30 px into the second turn's reply, so the line runs through it.
    await tc.evaluate((el) => {
      const reply = el.querySelectorAll('.chat-exchange')[1].querySelector('.response-content')!;
      el.scrollTop += reply.getBoundingClientRect().top - el.getBoundingClientRect().top + 30;
    });
    // A pixel of slack: WebKit rounds scrollTop, while the reply sits on a
    // fractional offset.
    await expect.poll(async () => Math.abs(((await replyTop(1)) ?? 1e6) + 30))
      .toBeLessThanOrEqual(1);
    const parked = (await replyTop(1))!;
    /** How far the reply has moved off where the reader parked it. */
    const drift = async () => Math.abs(((await replyTop(1)) ?? 1e6) - parked);
    // Past the save debounce, so the place is recorded before the reader leaves.
    await page.waitForTimeout(400);

    await newThread(page);
    await openThreadDrawer(page);
    await clickThreadRow(page, threadId);
    await ensureOnThreadPane(page);

    // The return lands on the line.
    await expect.poll(drift).toBeLessThanOrEqual(2);

    // The reader is typing when the late box arrives.
    const input = await waitForVisibleInput(page);
    await input.focus();

    // A box grows at the top of the same turn, above the reader's line.
    await tc.evaluate((el) => {
      const turn = el.querySelectorAll('.chat-exchange')[1] as HTMLElement;
      const late = document.createElement('div');
      late.style.height = '240px';
      turn.insertBefore(late, turn.firstChild);
    });

    // Held, within 2 px, once the transcript settles.
    await expect.poll(drift).toBeLessThanOrEqual(2);
    await page.waitForTimeout(600);
    expect(await drift()).toBeLessThanOrEqual(2);

    // No focus theft: the hold's writes leave the composer focused.
    expect(await input.evaluate(el => el === document.activeElement)).toBe(true);
  });
});
