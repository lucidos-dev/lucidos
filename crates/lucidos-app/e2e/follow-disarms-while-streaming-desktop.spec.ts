import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, disarmFollowSeed, followToggleState, pressFollowToggle } from './helpers';


const WHEEL_TRAVEL_PX = 200;

const TALL_MESSAGE = Array.from({ length: 30 }, (_, i) => `Context line ${i + 1}.`).join('\n');

test.describe('Scrolling away from a live reply retires the follow (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a wheel up while the agent is still writing turns the toggle off', async ({ page }) => {

    await page.setViewportSize({ width: 1280, height: 400 });
    await disarmFollowSeed(page);
    await navigateToApp(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const atBottom = () => tc.evaluate(el => el.scrollTop + el.clientHeight >= el.scrollHeight - 2);
    const overflow = () => tc.evaluate(el => el.scrollHeight - el.clientHeight);

    const agentIsWorking = () => page.evaluate(() => {
      const labels = document.querySelectorAll('.exchange-status-label');
      for (const label of labels) {
        const rect = label.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) continue;
        const text = label.textContent ?? '';
        if (text.includes('Working') || text.includes('Requesting')) return true;
      }
      return false;
    });

    await sendMessage(page, TALL_MESSAGE);
    await waitForResponse(page);
    expect(await overflow()).toBeGreaterThan(WHEEL_TRAVEL_PX + 20);

    await pressFollowToggle(page);
    await expect.poll(() => followToggleState(page)).toBe('true');
    await expect.poll(atBottom).toBe(true);

    await sendMessage(page, TALL_MESSAGE);
    await expect.poll(agentIsWorking, { timeout: 30_000 }).toBe(true);
    await expect.poll(atBottom).toBe(true);

    expect(await agentIsWorking(), 'the reply finished before the wheel could land').toBe(true);

    const box = await tc.boundingBox();
    if (!box) throw new Error('transcript has no box');
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.wheel(0, -WHEEL_TRAVEL_PX);

    await expect.poll(() => followToggleState(page), { timeout: 5_000 }).toBe('false');

    await expect.poll(atBottom).toBe(false);
    let previous = -1;
    await expect.poll(async () => {
      const now = await tc.evaluate(el => el.scrollTop);
      const held = now === previous;
      previous = now;
      return held;
    }).toBe(true);
    const parked = await tc.evaluate(el => el.scrollTop);
    await waitForResponse(page);
    await expect.poll(() => tc.evaluate(el => el.scrollTop)).toBe(parked);
    await expect(page.locator('button.scroll-to-bottom.visible')).toHaveCount(1);
  });

});
