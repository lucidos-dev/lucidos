import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, waitForResponse, assertHealthy, uniqueMessage,
  waitForScrollSettled, isMobileViewport, ensureOnThreadPane, disarmFollowSeed,
  enableMobileDynamicBars, disableMobileDynamicBars,
} from './helpers';

/** Answering a question card rests the reader on the live edge until the agent
 *  starts, follow armed or not (ADR 0080). What they wait to see is the agent's
 *  line under the card and the first thing the agent writes there. Reported
 *  from a phone with the follow off and dynamic bars on, where the reply landed
 *  under the composer. Every project runs it. */

const transcript = (page: Page) => page.locator('.thread-content.visible:visible').first();

/** How the answered card's turn sits against what covers the transcript, as
 *  the px hidden below the clear area. The clear area ends at the transcript's
 *  bottom, or at the composer's top where the composer overlays it. Two things
 *  are measured: the whole agent line (`.response-header`), and the FIRST line
 *  of the agent's reply. The reply streams on below the fold by design, so only
 *  its start is owed. -1 means the element is not there yet. */
function hiddenBelow(page: Page) {
  return transcript(page).evaluate((el) => {
    const answered = Array.from(el.querySelectorAll<HTMLElement>('.question-body-answered')).pop();
    const turn = answered?.closest<HTMLElement>('.chat-exchange');
    if (!turn) return { line: -1, reply: -1 };
    const box = Array.from(document.querySelectorAll<HTMLElement>('.prompt-box'))
      .find((b) => b.getBoundingClientRect().height > 0);
    const pane = el.getBoundingClientRect();
    const clearBottom = Math.min(pane.bottom, box ? box.getBoundingClientRect().top : pane.bottom);
    const hidden = (bottom: number) => Math.max(0, Math.round(bottom - clearBottom));
    const header = turn.querySelector<HTMLElement>('.response-header');
    const chunk = turn.querySelector<HTMLElement>('.response-chunk');
    const lineHeight = chunk ? parseFloat(getComputedStyle(chunk).lineHeight) || 20 : 0;
    const firstLine = chunk ? chunk.getBoundingClientRect().top + lineHeight : 0;
    return {
      line: header ? hidden(header.getBoundingClientRect().bottom) : -1,
      reply: chunk ? hidden(firstLine) : -1,
    };
  });
}

/** Two finished turns, so the transcript overflows. */
async function tallThread(page: Page) {
  await navigateToApp(page);
  await ensureOnThreadPane(page);
  await sendMessage(page, uniqueMessage('first'));
  await waitForResponse(page);
  await sendFollowUp(page, uniqueMessage('second'));
  await waitForResponse(page);
}

const QUESTION_TEXT = [
  'A long question, so the card is tall.',
  ...Array.from({ length: 8 }, (_, i) => `Paragraph ${i + 1} of the question text, long enough to wrap across the card.`),
  'Which way?',
].join('\\n\\n');

async function askQuestion(page: Page) {
  const args = `{"questions":[{"question":"${QUESTION_TEXT}","options":[{"label":"Yes"},{"label":"No"}]}]}`;
  await sendFollowUp(page, `${uniqueMessage('ask')} MOCK_TOOL_CALL: ask_user_question ${args}`);
  await expect(page.locator('.question-body:not(.question-body-answered):visible')).toHaveCount(1, { timeout: 15_000 });
}

/** The reader scrolls down to the options, the way they reach a tall card. A
 *  phone dispatches the swipe's `touchmove`, a desktop turns the wheel. */
async function readerScrollsToBottom(page: Page) {
  if (isMobileViewport(page)) {
    await transcript(page).evaluate((el) => {
      el.dispatchEvent(new Event('touchmove', { bubbles: true }));
      el.scrollTop = el.scrollHeight;
    });
  } else {
    const box = await transcript(page).boundingBox();
    if (!box) throw new Error('no transcript on screen');
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.wheel(0, 20_000);
  }
  await waitForScrollSettled(page);
}

async function expectAgentShown(page: Page) {
  await expect.poll(() => hiddenBelow(page), { timeout: 10_000 }).toEqual({ line: 0, reply: 0 });
  // And it stays: nothing after the landing takes it away again.
  await page.waitForTimeout(1500);
  expect(await hiddenBelow(page)).toEqual({ line: 0, reply: 0 });
}

test.describe('An answer shows the agent picking it up', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await disarmFollowSeed(page);
    if (isMobileViewport(page)) await enableMobileDynamicBars(page);
    else await page.setViewportSize({ width: 1280, height: 520 });
  });

  test.afterEach(async ({ page }) => {
    if (isMobileViewport(page)) await disableMobileDynamicBars(page);
  });

  test('a picked option lands on the agent line and its first reply', async ({ page }) => {
    await tallThread(page);
    await askQuestion(page);
    await readerScrollsToBottom(page);

    await page.locator('.question-body:not(.question-body-answered):visible .question-option').first().click();
    await expectAgentShown(page);
  });

  test('a typed answer lands on the agent line and its first reply', async ({ page }) => {
    await tallThread(page);
    await askQuestion(page);
    await readerScrollsToBottom(page);

    const input = page.locator('[data-role="prompt-input"]:visible').first();
    await input.fill(uniqueMessage('typed'));
    if (isMobileViewport(page)) await page.locator('button[aria-label="Submit answer"]:visible').first().click();
    else await input.press('Enter');
    await expectAgentShown(page);
  });
});
