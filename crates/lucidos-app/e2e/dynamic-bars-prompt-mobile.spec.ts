import { test, expect, type Page } from './fixtures';
import {
  assertHealthy,
  blurActiveElement,
  disableMobileDynamicBars,
  disarmFollowSeed,
  enableMobileDynamicBars,
  navigateToApp,
  sendMessage,
  uniqueMessage,
  waitForResponse,
} from './helpers';


const CONTAINER = '.mobile-swipe-pane .thread-content.visible';
const PROMPT = '.mobile-swipe-pane .prompt-area';

async function tallThreadAtTop(page: Page): Promise<void> {
  await navigateToApp(page);
  await sendMessage(page, `Say exactly: "${uniqueMessage('dynamic-bars')}"`);
  await waitForResponse(page);
  await blurActiveElement(page);
  await page.evaluate((sel) => {
    const container = document.querySelector(sel);
    if (!container) return;
    const filler = document.createElement('div');
    filler.style.minHeight = '3000px';
    filler.style.flexShrink = '0';
    filler.dataset.testFiller = 'true';
    container.appendChild(filler);
  }, CONTAINER);
  await page.waitForFunction((sel) => {
    const c = document.querySelector(sel);
    return !!c && c.scrollHeight - c.clientHeight > 2000;
  }, CONTAINER, { timeout: 5_000 });
  await page.evaluate((sel) => {
    const c = document.querySelector(sel);
    if (c) c.scrollTop = 0;
  }, CONTAINER);
  await page.waitForFunction((sel) => document.querySelector(sel)?.scrollTop === 0, CONTAINER);
}

async function scrollInSteps(page: Page, total: number): Promise<void> {
  await page.evaluate(async ({ sel, total }) => {
    const c = document.querySelector(sel);
    if (!c) return;
    const step = 80 * Math.sign(total);
    for (let moved = 0; Math.abs(moved) < Math.abs(total); moved += step) {
      const before = c.scrollTop;
      c.scrollTop = before + step;
      if (c.scrollTop === before) break;
      await new Promise((r) => setTimeout(r, 80));
    }
  }, { sel: CONTAINER, total });
}

function promptHiddenPx(page: Page): Promise<number> {
  return page.evaluate(({ sel, container }) => {
    const prompt = document.querySelector(sel);
    const pane = document.querySelector(container)?.closest('.thread-pane');
    if (!prompt || !pane) return -1;
    return prompt.getBoundingClientRect().bottom - pane.getBoundingClientRect().bottom;
  }, { sel: PROMPT, container: CONTAINER });
}

test.describe('Dynamic bars: the prompt', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await disarmFollowSeed(page);
  });

  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  test('glides away on scroll down, returns on scroll up, and shows at the end', async ({ page }) => {
    await enableMobileDynamicBars(page);
    await tallThreadAtTop(page);
    expect(await promptHiddenPx(page)).toBeLessThanOrEqual(1);
    const hitsPrompt = await page.evaluate((sel) => {
      const input = document.querySelector<HTMLElement>(`${sel} [data-role="prompt-input"]`);
      if (!input) return false;
      const r = input.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return !!hit?.closest(sel);
    }, PROMPT);
    expect(hitsPrompt).toBe(true);

    await scrollInSteps(page, 800);
    const promptHeight = await page.evaluate((sel) => document.querySelector(sel)!.getBoundingClientRect().height, PROMPT);
    await expect.poll(() => promptHiddenPx(page)).toBeGreaterThanOrEqual(promptHeight - 1);

    await scrollInSteps(page, -400);
    await expect.poll(() => promptHiddenPx(page)).toBeLessThanOrEqual(1);

    await scrollInSteps(page, 10_000);
    await expect.poll(() => promptHiddenPx(page)).toBeLessThanOrEqual(1);
    const gap = await page.evaluate(({ container, prompt }) => {
      const filler = document.querySelector(`${container} [data-test-filler]`);
      const box = document.querySelector(prompt);
      if (!filler || !box) return -1;
      return box.getBoundingClientRect().top - filler.getBoundingClientRect().bottom;
    }, { container: CONTAINER, prompt: PROMPT });
    expect(gap).toBeGreaterThanOrEqual(0);
  });

  test('never moves while the bars are pinned', async ({ page }) => {
    await disableMobileDynamicBars(page);
    await tallThreadAtTop(page);
    const before = await page.evaluate((sel) => document.querySelector(sel)!.getBoundingClientRect().top, PROMPT);

    await scrollInSteps(page, 800);

    const after = await page.evaluate((sel) => {
      const el = document.querySelector<HTMLElement>(sel)!;
      return { top: el.getBoundingClientRect().top, translate: el.style.translate, position: getComputedStyle(el).position };
    }, PROMPT);
    expect(after.translate).toBe('');
    expect(after.position).not.toBe('absolute');
    expect(after.top).toBeCloseTo(before, 0);
  });
});
