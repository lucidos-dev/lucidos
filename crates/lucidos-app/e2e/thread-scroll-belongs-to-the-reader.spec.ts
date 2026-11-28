import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, disarmFollowSeed, hideDashboardRail, followToggleState, pressFollowToggle } from './helpers';

test.describe('Transcript scroll belongs to the reader (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await hideDashboardRail(page);
  });

  test('a send rides nothing, the toggle rides, and taking it back moves nobody', async ({ page }) => {

    await page.setViewportSize({ width: 1280, height: 400 });
    await disarmFollowSeed(page);
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const atBottom = () => tc.evaluate(el => el.scrollTop + el.clientHeight >= el.scrollHeight - 2);
    const restingScrollTop = async () => {
      let last = Number.NaN;
      await expect.poll(async () => {
        const now = await tc.evaluate(el => el.scrollTop);
        const resting = now === last;
        last = now;
        return resting;
      }, { intervals: [150] }).toBe(true);
      return last;
    };
    const growLastTurn = () => tc.evaluate((el) => {
      const turns = el.querySelectorAll('.chat-exchange');
      const last = turns[turns.length - 1] as HTMLElement;
      const grown = document.createElement('div');
      grown.style.height = '600px';
      last.appendChild(grown);
    });

    await sendMessage(page, 'List the numbers from 1 to 40, one per line, and nothing else.');
    await waitForResponse(page);
    await expect.poll(() => tc.evaluate(el => el.scrollHeight - el.clientHeight)).toBeGreaterThan(100);
    await expect.poll(atBottom).toBe(false);
    const answer = await tc.evaluate((el) => {
      const chunks = el.querySelectorAll('.chat-exchange .response-chunk');
      if (!chunks.length) throw new Error('the reply drew no prose block');
      const top = chunks[0].getBoundingClientRect().top;
      const bottom = chunks[chunks.length - 1].getBoundingClientRect().bottom;
      const pane = el.getBoundingClientRect();
      return {
        shown: Math.max(0, Math.min(bottom, pane.bottom) - Math.max(top, pane.top)),
        height: bottom - top,
      };
    });
    expect(answer.shown).toBeLessThan(answer.height / 4);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(1);

    await sendMessage(page, 'List the numbers from 1 to 20, one per line, and nothing else.');
    await waitForResponse(page);
    const landed = await restingScrollTop();
    expect(landed).toBeGreaterThan(50);
    await expect.poll(atBottom).toBe(false);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(1);
    await growLastTurn();
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBe(landed);

    await pressFollowToggle(page);
    await expect.poll(() => followToggleState(page)).toBe('true');
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);

    await sendMessage(page, 'List the numbers from 1 to 30, one per line, and nothing else.');
    await waitForResponse(page);
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);

    await page.waitForTimeout(1500);
    await growLastTurn();
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);
    await expect.poll(() => followToggleState(page)).toBe('true');

    const riding = await tc.evaluate(el => el.scrollTop);
    await pressFollowToggle(page);
    await expect.poll(() => followToggleState(page)).toBe('false');
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBe(riding);

    await growLastTurn();
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBe(riding);
    await expect.poll(atBottom).toBe(false);

    await page.locator('button.scroll-to-bottom.visible').click();
    await expect.poll(atBottom).toBe(true);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);
    await expect.poll(() => followToggleState(page)).toBe('false');
  });

  test('a device that has never pressed the toggle rides a brand-new thread', async ({ page }) => {

    await page.setViewportSize({ width: 1280, height: 400 });
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const atBottom = () => tc.evaluate(el => el.scrollTop + el.clientHeight >= el.scrollHeight - 2);

    await expect.poll(() => followToggleState(page)).toBe('true');

    await sendMessage(page, 'List the numbers from 1 to 40, one per line, and nothing else.');
    await waitForResponse(page);
    await expect.poll(() => tc.evaluate(el => el.scrollHeight - el.clientHeight)).toBeGreaterThan(100);
    await expect.poll(atBottom).toBe(true);
    await expect.poll(() => followToggleState(page)).toBe('true');
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(0);
  });
});
