import { test, expect } from './fixtures';
import {
  assertHealthy, clickThreadRow, disarmFollowSeed, ensureOnThreadPane, hideDashboardRail,
  navigateToApp, newThread, openThreadDrawer, sendMessageCapturingThreadId, sendMessage,
  waitForResponse, waitForVisibleInput, waitForVisibleResponseCount,
} from './helpers';

test.describe('a return holds the line the reader left', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await hideDashboardRail(page);
  });

  test('the reply the reader was reading stays put while the turn above it grows', async ({ page }) => {
    const width = page.viewportSize()?.width ?? 1280;
    await page.setViewportSize({ width, height: 420 });
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
    const replyTop = (index: number) => tc.evaluate((el, i) => {
      const turn = el.querySelectorAll('.chat-exchange')[i];
      const reply = turn?.querySelector('.response-content');
      if (!reply) return null;
      return reply.getBoundingClientRect().top - el.getBoundingClientRect().top;
    }, index);

    await tc.evaluate((el) => {
      const reply = el.querySelectorAll('.chat-exchange')[1].querySelector('.response-content')!;
      el.scrollTop += reply.getBoundingClientRect().top - el.getBoundingClientRect().top + 30;
    });
    await expect.poll(async () => Math.abs(((await replyTop(1)) ?? 1e6) + 30))
      .toBeLessThanOrEqual(1);
    const parked = (await replyTop(1))!;
    const drift = async () => Math.abs(((await replyTop(1)) ?? 1e6) - parked);
    await page.waitForTimeout(400);

    await newThread(page);
    await openThreadDrawer(page);
    await clickThreadRow(page, threadId);
    await ensureOnThreadPane(page);

    await expect.poll(drift).toBeLessThanOrEqual(2);

    const input = await waitForVisibleInput(page);
    await input.focus();

    await tc.evaluate((el) => {
      const turn = el.querySelectorAll('.chat-exchange')[1] as HTMLElement;
      const late = document.createElement('div');
      late.style.height = '240px';
      turn.insertBefore(late, turn.firstChild);
    });

    await expect.poll(drift).toBeLessThanOrEqual(2);
    await page.waitForTimeout(600);
    expect(await drift()).toBeLessThanOrEqual(2);

    expect(await input.evaluate(el => el === document.activeElement)).toBe(true);
  });
});
