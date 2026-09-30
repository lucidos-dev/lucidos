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

/** Dynamic bars move the prompt as well as the header: it slides down on a
 *  scroll down, comes back on a scroll up, and is fully shown at the end of the
 *  thread. With the bars pinned it never moves. `hooks/useHideOnScroll.ts`. */

const CONTAINER = '.mobile-swipe-pane .thread-content.visible';
const PROMPT = '.mobile-swipe-pane .prompt-area';

/** A thread tall enough to scroll, parked at the top. The filler makes the
 *  height independent of how long the model's reply is. */
async function tallThreadAtTop(page: Page): Promise<void> {
  await navigateToApp(page);
  await sendMessage(page, `Say exactly: "${uniqueMessage('dynamic-bars')}"`);
  await waitForResponse(page);
  // The hook holds everything still while an input in the pane has focus.
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

/** Scroll by `total` px in spaced steps, the way a drag does. One jump is one
 *  event, and it can land inside the app's own navigation window and be
 *  ignored (see thread-title.spec.ts). */
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

/** How much of the prompt sits below the bottom of the pane, in px. */
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
    // The follow seed ships armed, and a rider is carried back to the live edge
    // by any scroll that is not their own gesture. These tests park at the top.
    await disarmFollowSeed(page);
  });

  // The pref is global and the e2e database resets only between projects.
  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  test('slides away on scroll down, returns on scroll up, and shows at the end', async ({ page }) => {
    await enableMobileDynamicBars(page);
    await tallThreadAtTop(page);
    expect(await promptHiddenPx(page)).toBeLessThanOrEqual(1);
    // The overlaid prompt takes taps, not the transcript it sits over.
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
    // Nothing at the end of the transcript sits under the prompt.
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
      return { top: el.getBoundingClientRect().top, transform: el.style.transform, position: getComputedStyle(el).position };
    }, PROMPT);
    expect(after.transform).toBe('');
    expect(after.position).not.toBe('absolute');
    expect(after.top).toBeCloseTo(before, 0);
  });
});
