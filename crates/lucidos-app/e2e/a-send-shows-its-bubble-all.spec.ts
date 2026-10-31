import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, waitForResponse, assertHealthy,
  uniqueMessage, waitForScrollSettled, isMobileViewport, ensureOnThreadPane, waitForVisibleInput,
} from './helpers';

/** A send rests the reader on the live edge, so the bubble they just wrote is
 *  in view, whatever the thread is doing (ADR 0080). The reader scrolls up
 *  first in every case, so the landing has somewhere to go. The three cases are
 *  the states a report named: a turn still running, a question card just
 *  cancelled, and a typed answer.
 *
 *  The mock keeps a turn running with a scripted `run_python` sleep, and asks a
 *  question card through a scripted `ask_user_question` call. Every project
 *  runs it: a phone's closing keyboard and dynamic bars write the offset too. */

const transcript = (page: Page) => page.locator('.thread-content.visible:visible').first();

/** How much of the user bubble carrying `marker` the transcript shows. A typed
 *  answer to a question renders inside the card, so its block counts too. */
function bubbleShown(page: Page, marker: string) {
  return transcript(page).evaluate((el, text) => {
    const panels = Array.from(el.querySelectorAll<HTMLElement>('.initiator-panel-user, .question-freetext-text'));
    const panel = panels.reverse().find((p) => (p.textContent ?? '').includes(text));
    if (!panel) return -1;
    const pane = el.getBoundingClientRect();
    const box = panel.getBoundingClientRect();
    return Math.max(0, Math.min(box.bottom, pane.bottom) - Math.max(box.top, pane.top)) / Math.max(1, box.height);
  }, marker);
}

/** Scroll the transcript up the way a reader does. A real wheel on desktop. A
 *  phone has no wheel, so there it dispatches the swipe's `touchmove` and moves
 *  the offset. That is all a swipe amounts to for the transcript. */
async function readerScrollsUp(page: Page, by: number) {
  if (isMobileViewport(page)) {
    await transcript(page).evaluate((el, dy) => {
      el.dispatchEvent(new Event('touchmove', { bubbles: true }));
      el.scrollTop -= dy;
    }, by);
  } else {
    const box = await transcript(page).boundingBox();
    if (!box) throw new Error('no transcript on screen');
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.wheel(0, -by);
  }
  await waitForScrollSettled(page);
}

async function expectBubbleShown(page: Page, marker: string) {
  await expect.poll(() => bubbleShown(page, marker), { timeout: 5_000 }).toBeGreaterThan(0.9);
  // And it stays: nothing after the landing takes it away again.
  await page.waitForTimeout(1500);
  expect(await bubbleShown(page, marker)).toBeGreaterThan(0.9);
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

test.describe('A send shows the reader their own bubble', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    if (!isMobileViewport(page)) await page.setViewportSize({ width: 1280, height: 520 });
  });

  test('a follow-up sent while a turn runs lands on its bubble', async ({ page }) => {
    await tallThread(page);
    // Long enough to outlast the scroll and both checks on a slow host.
    await sendFollowUp(page, `${uniqueMessage('busy')} MOCK_RUN_PYTHON:import time; time.sleep(12)`);
    await expect(page.locator('button.send-cancel-morph[aria-label="Cancel"]:not(:disabled):visible')).toHaveCount(1, { timeout: 15_000 });
    await readerScrollsUp(page, 600);

    const marker = uniqueMessage('q');
    await sendFollowUp(page, marker);
    // Queued proves the turn was still running when the follow-up went out.
    await expect(page.locator('.exchange-status-queued:visible')).toHaveCount(1, { timeout: 5_000 });
    await expectBubbleShown(page, marker);
  });

  test('a send after cancelling a question card lands on its bubble', async ({ page }) => {
    await tallThread(page);
    await askQuestion(page);
    await readerScrollsUp(page, 600);
    await page.locator('button.action-btn-danger[aria-label="Cancel"]:not(:disabled):visible').first().click();
    await expect(page.locator('.question-cancel-picked:visible')).toHaveCount(1, { timeout: 15_000 });
    await expect(page.locator('button.send-cancel-morph[aria-label="Cancel"]:visible')).toHaveCount(0, { timeout: 15_000 });
    await readerScrollsUp(page, 300);

    const marker = uniqueMessage('q');
    await sendFollowUp(page, marker);
    await expectBubbleShown(page, marker);
  });

  test('an answer typed into a question card lands on it', async ({ page }) => {
    await tallThread(page);
    await askQuestion(page);
    await readerScrollsUp(page, 600);

    // A pending question turns Send into Submit, which a phone has to press.
    const marker = uniqueMessage('q');
    const input = await waitForVisibleInput(page);
    await input.fill(marker);
    if (isMobileViewport(page)) await page.locator('button[aria-label="Submit answer"]:visible').first().click();
    else await input.press('Enter');
    await expectBubbleShown(page, marker);
  });
});
