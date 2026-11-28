import { randomUUID } from 'node:crypto';
import { psql, seedChatThread } from './db-helpers';
import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, waitForResponse, assertHealthy, uniqueMessage,
  waitForScrollSettled, isMobileViewport, ensureOnThreadPane, disarmFollowSeed,
  enableMobileDynamicBars, disableMobileDynamicBars,
} from './helpers';


const transcript = (page: Page) => page.locator('.thread-content.visible:visible').first();

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

const queuedWakeAnswers = [
  { kind: 'Selected', option_id: 'yes' },
  { kind: 'MultiSelected', option_ids: ['yes'] },
  { kind: 'FreeText', text: 'Yes, please continue.' },
] as const;

for (const answer of queuedWakeAnswers) {
  test(`a queued wake reply stays below the ${answer.kind} card on reload`, async ({ page }) => {
    const threadId = seedChatThread({ turns: 1, needles: [], title: 'Queued wake reply' });
    const questionId = randomUUID();
    const wakeId = randomUUID();
    const toolUseId = randomUUID();
    const reply = 'The queued wake ran after the answer.';
    const base = Date.now();
    const events = [
      [questionId, 'UserQuestionAsked', {
        tool_use_id: toolUseId, cc_session_id: '', channel: 'chat',
        question: 'Continue?', options: [{ id: 'yes', label: 'Yes' }], multi_select: answer.kind === 'MultiSelected',
      }],
      [wakeId, 'PromptInjected', {
        mode: 'agent', text: 'The event arrived.',
        origin: { kind: 'engine', reason: { kind: 'event_wait', outcome: 'delivered', watched: ['JobCompleted'], wait_reason: 'the job to finish' } },
      }],
      [randomUUID(), 'UserQuestionAnswered', {
        tool_use_id: toolUseId, channel: 'chat', answer,
      }],
      [randomUUID(), 'TextStreamed', { text: reply, request_event_id: wakeId }],
      [randomUUID(), 'ResponseGenerated', { text: reply, request_event_id: wakeId, images: [] }],
    ];
    try {
      const rows = events.map(([id, type, payload], i) =>
        `('${id}', '${type}', '${JSON.stringify(payload)}'::jsonb, '${new Date(base + i).toISOString()}', 'thread', '${threadId}', '${threadId}')`);
      psql(`INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ${rows.join(',')}`);

      await page.goto(`/#thread=${threadId}`);
      for (const load of ['first open', 'reload']) {
        if (load === 'reload') await page.reload();
        await ensureOnThreadPane(page);
        const card = transcript(page).locator('.question-body-answered').filter({ hasText: 'Continue?' });
        const response = transcript(page).locator('.response-chunk').getByText(reply, { exact: true });
        if (answer.kind === 'FreeText') {
          await expect(card.getByText(answer.text, { exact: true })).toBeVisible();
        } else {
          await expect(card.locator('.question-option-static.question-option-selected')).toHaveText('Yes');
        }
        await expect(response).toBeVisible();
        const cardBox = await card.boundingBox();
        const replyBox = await response.boundingBox();
        expect(replyBox!.y, load).toBeGreaterThanOrEqual(cardBox!.y + cardBox!.height);
      }
    } finally {
      psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}';`);
    }
  });
}
