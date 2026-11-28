import type { Locator, Page } from '@playwright/test';
import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, uniqueMessage, assertHealthy,
  pickComposeDestination, newThread, countExchanges, waitForActionPanel,
  waitForResponse, waitForVisibleInput, waitForStreamingToStart, isMobileViewport,
  openThreadDrawer, ensureOnThreadPane, waitForPaneAtRest, composerControl,
} from './helpers';
import { psql, createCCThreadWithChange, cleanupCCThread } from './db-helpers';

test.use({ serviceWorkers: 'block' });

function seedWaitingCard(name: string, multiSelect = false) {
  const suffix = randomUUID().slice(0, 8);
  const toolUseId = `tu-side-${suffix}`;
  const { threadId, changeId, branch, file } = createCCThreadWithChange(name, suffix);
  const question = JSON.stringify({
    tool_use_id: toolUseId,
    cc_session_id: '',
    channel: 'claude_code',
    multi_select: multiSelect,
    question: `Option one, or option two? ${suffix}`,
    options: [
      { id: 'one', label: 'Option one', description: 'The first.' },
      { id: 'two', label: 'Option two', description: 'The second.' },
    ],
  }).replace(/'/g, "''");
  psql([
    `UPDATE thread_summaries SET status = 'waiting_for_user_answer' WHERE thread_id = '${threadId}'`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${question}'::jsonb, '${new Date().toISOString()}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));
  return { threadId, changeId, branch, file, toolUseId, title: `${name} ${suffix}` };
}

async function pressSideQuestionToggle(page: Page): Promise<void> {
  await (await composerControl(page, '[data-role="side-question-toggle"]')).click();
}

const roundAsk = (page: Page): Locator =>
  page.locator('button.send-cancel-morph[aria-label="Ask side question"]:visible').first();

async function askByToggle(page: Page, question: string): Promise<void> {
  const input = await waitForVisibleInput(page, 15_000);
  await input.fill(question);
  await pressSideQuestionToggle(page);
  await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
  await roundAsk(page).click();
}

test.describe('side questions in a Claude Code thread', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a side question answers on a card and never enters the thread', async ({ page }) => {
    const chatPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && /\/api\/v1\/chat\b/.test(req.url())) chatPosts.push(req.url());
    });

    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);
    const token = uniqueMessage('side-codeword');
    await sendMessage(page, `Say exactly: "codeword ${token}" and nothing else. Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);
    const exchangesBefore = await countExchanges(page);
    const postsBefore = chatPosts.length;

    const sideToken = uniqueMessage('side-answer');
    const question = `Say exactly: "${sideToken}" and nothing else.`;
    await askByToggle(page, question);

    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card).toBeVisible();
    await expect(card.locator('.side-question-question')).toHaveText(
      question,
    );
    await expect(card).toHaveAttribute('data-status', 'answered', { timeout: 150_000 });
    await expect(card.locator('.side-question-body > .markdown-content')).toContainText(sideToken);
    await expect(card).toContainText('Not added to the conversation');

    expect(await countExchanges(page)).toBe(exchangesBefore);
    expect(chatPosts.length).toBe(postsBefore);

    const head = card.locator('button.side-question-head');
    await expect(head).toHaveAttribute('aria-expanded', 'true');
    const box = (await head.boundingBox())!;
    const centre = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    for (const dy of [-21, 21]) {
      const hit = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)
        ?.closest('button')?.classList.contains('side-question-head') ?? false, { x: centre.x, y: centre.y + dy });
      expect(hit).toBe(true);
    }
    const dismissed = page.waitForResponse((res) => res.url().includes('/api/v1/side-questions/dismiss'));
    await head.click();
    expect((await dismissed).ok()).toBe(true);
    await expect(card).toHaveAttribute('data-collapsed', '');
    await expect(head).toHaveAttribute('aria-expanded', 'false');
    await expect(card.locator('.side-question-summary')).toContainText(question);
    await expect(card.locator('.side-question-question')).toHaveCount(0);

    await page.reload();
    const folded = page.locator('[data-role="side-question-card"][data-collapsed]:visible').first();
    await expect(folded).toBeVisible({ timeout: 30_000 });
    await folded.locator('button.side-question-head').click();
    await expect(page.locator('[data-role="side-question-card"]:visible .side-question-body > .markdown-content')).toContainText(sideToken);
    expect(await countExchanges(page)).toBe(exchangesBefore);
  });

  test('the toggle turns on the mode and the round Ask leaves the card waiting', async ({ page }) => {
    const { threadId, changeId, branch, file, toolUseId, title } = seedWaitingCard('E2E Side Submit');

    const sideQuestionBodies: string[] = [];
    await page.route('**/api/v1/side-questions', (route) => {
      sideQuestionBodies.push(route.request().postData() ?? '');
      return route.fulfill({
        status: 400,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Refused by the test.' }),
      });
    });
    const answerPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && req.url().includes('/answer-question')) answerPosts.push(req.url());
    });

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("${title}")`).first().click();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);
      await expect(page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first())
        .toBeVisible({ timeout: 15_000 });

      const input = await waitForVisibleInput(page, 15_000);
      await input.fill('what does option two mean?');
      const submit = page.locator('button[aria-label="Submit answer"]:visible').first();
      await expect(submit).toBeVisible({ timeout: 10_000 });
      await pressSideQuestionToggle(page);

      await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
      await expect(input).toHaveValue('what does option two mean?');
      await expect(roundAsk(page)).toBeVisible();
      await expect(page.locator('button[aria-label="Submit answer"]:visible')).toHaveCount(0);
      expect(answerPosts).toEqual([]);

      await roundAsk(page).click();
      await expect.poll(() => sideQuestionBodies.length).toBe(1);
      expect(sideQuestionBodies[0]).toContain('what does option two mean?');
      expect(answerPosts).toEqual([]);
      await expect(page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first()).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('the toggle on a waiting card starts a side question', async ({ page }) => {
    const { threadId, changeId, branch, file, toolUseId, title } = seedWaitingCard('E2E Side Cancel');
    const sideQuestionBodies: string[] = [];
    await page.route('**/api/v1/side-questions', (route) => {
      sideQuestionBodies.push(route.request().postData() ?? '');
      return route.fulfill({
        status: 400,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Refused by the test.' }),
      });
    });
    const answerOrStopPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() !== 'POST') return;
      if (/\/answer-question|\/claude-code\/stop|\/chat\/cancel/.test(req.url())) answerOrStopPosts.push(req.url());
    });

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("${title}")`).first().click();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);
      const card = page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first();
      await expect(card).toBeVisible({ timeout: 15_000 });

      const input = await waitForVisibleInput(page, 15_000);
      await expect(input).toHaveValue('');
      const cancel = page.locator('button.action-btn-danger[aria-label="Cancel"]:not(:disabled):visible').first();
      await expect(cancel).toBeVisible({ timeout: 10_000 });
      await pressSideQuestionToggle(page);

      await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
      await expect(input).toHaveAttribute('placeholder', 'Ask a side question…');
      expect(answerOrStopPosts, 'turning the mode on cancelled nothing').toEqual([]);

      await input.fill('what does option one mean?');
      await roundAsk(page).click();
      await expect.poll(() => sideQuestionBodies.length).toBe(1);
      expect(sideQuestionBodies[0]).toContain('what does option one mean?');
      expect(answerOrStopPosts).toEqual([]);
      await expect(card).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('the toggle turns on the mode on a multi-select card, even before a pick', async ({ page }) => {
    const { threadId, changeId, branch, file, toolUseId, title } = seedWaitingCard('E2E Side Multi Hold', true);
    const answerPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && req.url().includes('/answer-question')) answerPosts.push(req.url());
    });
    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("${title}")`).first().click();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);
      const card = page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first();
      await expect(card).toBeVisible({ timeout: 15_000 });

      const cancel = page.locator('button.action-btn-danger[aria-label="Cancel"]:visible');
      await expect(cancel).toBeVisible({ timeout: 10_000 });
      await expect(page.locator('button[aria-label="Submit answer"]:visible')).toHaveCount(0);
      await pressSideQuestionToggle(page);
      await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
      await expect(page.locator('button[aria-label="Submit answer"]:visible')).toHaveCount(0);
      expect(answerPosts).toEqual([]);
      await expect(card).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('the Side question shortcut works on a multi-select card', async ({ page }) => {
    test.skip(isMobileViewport(page), 'Keyboard shortcuts are desktop only');
    const { threadId, changeId, branch, file, toolUseId, title } = seedWaitingCard('E2E Side Multi', true);
    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("${title}")`).first().click();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);
      const card = page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first();
      await expect(card).toBeVisible({ timeout: 15_000 });

      await expect(page.locator('button.action-btn-danger[aria-label="Cancel"]:visible')).toBeVisible({ timeout: 10_000 });
      const input = await waitForVisibleInput(page, 15_000);
      await input.focus();
      await page.keyboard.press('Alt+Enter');
      await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
      await page.keyboard.type('what does option two mean?');
      await expect(roundAsk(page)).toBeVisible();
      await expect(page.locator('button[aria-label="Submit answer"]:visible')).toHaveCount(0);
      await expect(card).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('a failed side question says why on the card', async ({ page }) => {
    await page.route('**/api/v1/side-questions', (route) => route.fulfill({
      status: 400,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'Side questions are not available in Codex threads.' }),
    }));
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);
    await sendMessage(page, `Say exactly: "${uniqueMessage('side-refusal')}". Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);

    await askByToggle(page, 'anything');
    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card).toHaveAttribute('data-status', 'failed');
    await expect(card.getByRole('alert')).toHaveText('Side questions are not available in Codex threads.');
  });
});

test.describe('side questions in a Lucidos Agent thread', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the toggle turns on the mode with the draft kept, and Ask asks it', async ({ page }) => {
    const chatPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && /\/api\/v1\/chat\b/.test(req.url())) chatPosts.push(req.url());
    });

    await navigateToApp(page);
    await newThread(page);
    await sendMessage(page, uniqueMessage('lucidos-side-question'));
    await waitForResponse(page);
    const exchangesBefore = await countExchanges(page);
    const postsBefore = chatPosts.length;

    const input = await waitForVisibleInput(page, 15_000);
    await input.fill('what did you just say?');
    await pressSideQuestionToggle(page);

    await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
    await expect(input).toHaveValue('what did you just say?');
    expect(chatPosts.length).toBe(postsBefore);
    await roundAsk(page).click();

    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card.locator('.side-question-question')).toHaveText('what did you just say?');
    await expect(card).toHaveAttribute('data-status', 'answered', { timeout: 60_000 });
    await expect(input).toHaveValue('');
    await expect(page.locator('[data-role="side-question-mode"]:visible')).toHaveCount(0);
    expect(await countExchanges(page)).toBe(exchangesBefore);
    expect(chatPosts.length).toBe(postsBefore);
  });

  test('typing on after the toggle, then Enter, asks the whole draft', async ({ page }) => {
    test.skip(isMobileViewport(page), 'Enter sends only on desktop; on a phone it is a newline');
    const chatPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && /\/api\/v1\/chat\b/.test(req.url())) chatPosts.push(req.url());
    });

    await navigateToApp(page);
    await newThread(page);
    await sendMessage(page, uniqueMessage('lucidos-side-question-typing'));
    await waitForResponse(page);
    const exchangesBefore = await countExchanges(page);
    const postsBefore = chatPosts.length;

    const input = await waitForVisibleInput(page, 15_000);
    await input.fill('what did');
    await input.focus();
    await pressSideQuestionToggle(page);

    await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
    await expect(input).toBeFocused();
    await page.keyboard.type(' you just say?');
    await expect(input).toHaveValue('what did you just say?');
    await page.keyboard.press('Enter');

    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card.locator('.side-question-question')).toHaveText('what did you just say?');
    await expect(input).toHaveValue('');
    expect(await countExchanges(page)).toBe(exchangesBefore);
    expect(chatPosts.length).toBe(postsBefore);
  });

  test('the Side question shortcut toggles the mode, even over an idle empty box', async ({ page }) => {
    test.skip(isMobileViewport(page), 'Keyboard shortcuts are desktop only');
    const sideQuestionPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && req.url().includes('/api/v1/side-questions')) sideQuestionPosts.push(req.url());
    });

    await navigateToApp(page);
    await newThread(page);
    await sendMessage(page, uniqueMessage('lucidos-side-question-toggle'));
    await waitForResponse(page);

    const input = await waitForVisibleInput(page, 15_000);
    const pill = page.locator('[data-role="side-question-mode"]:visible');
    await expect(input).toHaveValue('');
    await input.focus();
    await page.keyboard.press('Alt+Enter');
    await expect(pill).toBeVisible();
    await expect(input).toHaveAttribute('placeholder', 'Ask a side question…');
    await expect(input).toBeFocused();
    await page.keyboard.type('what did you just say?');
    await page.keyboard.press('Alt+Enter');
    await expect(pill).toHaveCount(0);
    await expect(input).toBeFocused();
    await expect(input).toHaveValue('what did you just say?');
    expect(sideQuestionPosts).toEqual([]);
  });

  test('the toggle turns on side-question mode and leaves the turn running', async ({ page }) => {
    const cancelPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && /\/api\/v1\/chat\/cancel\b/.test(req.url())) cancelPosts.push(req.url());
    });

    await navigateToApp(page);
    await newThread(page);
    await sendMessage(page, `Write an extremely long and detailed essay about the history of bridges. Be as verbose as possible. Include: ${uniqueMessage('stop-hold')}`);
    await waitForStreamingToStart(page, 5, 60_000);

    const stop = page.locator('button.send-cancel-morph[aria-label="Cancel"]:not(:disabled):visible').first();
    await expect(stop).toBeVisible({ timeout: 30_000 });
    await pressSideQuestionToggle(page);

    const input = await waitForVisibleInput(page, 15_000);
    const pill = page.locator('[data-role="side-question-mode"]:visible');
    await expect(pill).toBeVisible();
    await expect(input).toHaveValue('');
    await expect(input).toHaveAttribute('placeholder', 'Ask a side question…');
    await expect(input).toBeFocused();
    await expect(stop).toBeVisible();

    const saved = page.waitForResponse((res) => res.url().includes('/compose')
      && res.request().method() === 'PUT' && (res.request().postData() ?? '').includes('"sideQuestionMode":true'));
    await page.keyboard.type('what');
    await expect(input).toHaveValue('what');
    expect((await saved).ok()).toBe(true);

    await page.reload();
    const reloaded = await waitForVisibleInput(page, 30_000);
    await expect(pill).toBeVisible();
    await expect(reloaded).toHaveValue('what');

    const cleared = page.waitForResponse((res) => res.url().includes('/compose')
      && res.request().method() === 'PUT' && (res.request().postData() ?? '').includes('"sideQuestionMode":false'));
    await pill.getByRole('button', { name: 'Back to a normal message' }).click();
    await expect(pill).toHaveCount(0);
    await expect(reloaded).toHaveValue('what');
    expect((await cleared).ok()).toBe(true);
    await page.reload();
    await expect(await waitForVisibleInput(page, 30_000)).toHaveValue('what');
    await expect(pill).toHaveCount(0);
    expect(cancelPosts).toEqual([]);
  });

  test('the Side question shortcut over an empty box lets the user type and ask', async ({ page }) => {
    test.skip(isMobileViewport(page), 'Keyboard shortcuts are desktop only');
    const chatPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && /\/api\/v1\/chat\b/.test(req.url())) chatPosts.push(req.url());
    });

    await navigateToApp(page);
    await newThread(page);
    await sendMessage(page, `Write an extremely long and detailed essay about the history of bridges. Be as verbose as possible. Include: ${uniqueMessage('shortcut-empty')}`);
    await waitForStreamingToStart(page, 5, 60_000);
    const postsBefore = chatPosts.length;

    const input = await waitForVisibleInput(page, 15_000);
    const pill = page.locator('[data-role="side-question-mode"]:visible');
    await expect(input).toHaveValue('');
    await input.focus();
    await page.keyboard.press('Alt+Enter');
    await expect(pill).toBeVisible();
    await expect(input).toBeFocused();

    await page.keyboard.type('draft');
    await page.keyboard.press('Escape');
    await expect(pill).toHaveCount(0);
    await expect(input).toHaveValue('draft');
    expect(chatPosts.length).toBe(postsBefore);

    await input.fill('');
    await page.keyboard.press('Alt+Enter');
    await expect(pill).toBeVisible();
    await page.keyboard.type('what are you writing about?');
    await page.keyboard.press('Enter');

    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card.locator('.side-question-question')).toHaveText('what are you writing about?');
    await expect(input).toHaveValue('');
    await expect(pill).toHaveCount(0);
    expect(chatPosts.length).toBe(postsBefore);
  });

  test('a long press on Send sends the message and never turns on the mode', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await sendMessage(page, uniqueMessage('lucidos-send-long-press'));
    await waitForResponse(page);
    const exchangesBefore = await countExchanges(page);

    const input = await waitForVisibleInput(page, 15_000);
    await input.fill(uniqueMessage('lucidos-send-long-press-two'));
    const send = page.locator('[aria-label="Send message"]:visible').first();
    const box = (await send.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.waitForTimeout(700);
    await expect(page.locator('[data-role="side-question-mode"]:visible')).toHaveCount(0);
    await page.mouse.up();

    await expect.poll(() => countExchanges(page)).toBe(exchangesBefore + 1);
    await expect(page.locator('[data-role="side-question-mode"]:visible')).toHaveCount(0);
  });
});
