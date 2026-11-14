import { test, expect } from './fixtures';
import {
  assertHealthy,
  disableMobileDynamicBars,
  enableMobileDynamicBars,
  navigateToApp,
  sendMessage,
  uniqueMessage,
  waitForResponse,
} from './helpers';

/** The first send swaps the compose view for the thread view beside the
 *  prompt. The prompt must keep its DOM node through that swap, because
 *  useHideOnScroll measures that node as the transcript's bottom padding.
 *  Measuring any other node pushes the first turn out of view. `ThreadPane.tsx`. */

const PROMPT = '.mobile-swipe-pane .prompt-area';

type PromptHeightWrite = { publishedPx: number; promptPx: number };

test.describe('First send with dynamic bars', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  // The pref is global and the e2e database resets only between projects.
  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  test('keeps the prompt node, so the transcript reserves only its height', async ({ page }) => {
    await enableMobileDynamicBars(page);
    // Record every write of the prompt height beside the prompt's real height.
    await page.addInitScript((promptSel) => {
      const writes: PromptHeightWrite[] = [];
      (window as unknown as { __promptHeightWrites: PromptHeightWrite[] }).__promptHeightWrites = writes;
      const setProperty = CSSStyleDeclaration.prototype.setProperty;
      CSSStyleDeclaration.prototype.setProperty = function (name: string, value: string | null, priority?: string) {
        if (name === '--mobile-prompt-height' && value) {
          const rem = parseFloat(getComputedStyle(document.documentElement).fontSize);
          const prompt = document.querySelector(promptSel);
          writes.push({
            publishedPx: parseFloat(value) * rem,
            promptPx: prompt ? prompt.getBoundingClientRect().height : -1,
          });
        }
        return setProperty.call(this, name, value, priority);
      };
    }, PROMPT);
    await navigateToApp(page);
    await page.evaluate((sel) => {
      document.querySelector<HTMLElement>(sel)!.dataset.e2eComposePrompt = 'true';
    }, PROMPT);

    await sendMessage(page, `Say exactly: "${uniqueMessage('first-send-prompt')}"`);
    await waitForResponse(page);

    const promptKept = await page.evaluate(
      (sel) => document.querySelector<HTMLElement>(sel)?.dataset.e2eComposePrompt === 'true',
      PROMPT,
    );
    expect(promptKept).toBe(true);

    const writes = await page.evaluate(
      () => (window as unknown as { __promptHeightWrites: PromptHeightWrite[] }).__promptHeightWrites,
    );
    expect(writes.length).toBeGreaterThan(0);
    for (const write of writes) {
      expect(write.publishedPx, JSON.stringify(write)).toBeLessThanOrEqual(write.promptPx + 1);
    }

    // The turn the send created is on screen, above the prompt.
    const turnVisible = await page.evaluate((sel) => {
      const turn = document.querySelector('.mobile-swipe-pane .thread-content.visible .chat-exchange');
      const prompt = document.querySelector(sel);
      if (!turn || !prompt) return false;
      const t = turn.getBoundingClientRect();
      return t.bottom > 0 && t.top < prompt.getBoundingClientRect().top;
    }, PROMPT);
    expect(turnVisible).toBe(true);
  });
});
