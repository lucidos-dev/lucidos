import type { Locator, Page } from '@playwright/test';
import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, uniqueMessage, assertHealthy,
  pickComposeDestination, newThread, countExchanges, waitForActionPanel,
  waitForResponse, waitForVisibleInput, waitForStreamingToStart, isMobileViewport,
  openThreadDrawer, ensureOnThreadPane, waitForPaneAtRest,
} from './helpers';
import { psql, createCCThreadWithChange, cleanupCCThread } from './db-helpers';

// A controlled page sends fetches through the service worker in WebKit, where
// `page.route` cannot see them. `sw.js` handles GETs only, so blocking it
// changes nothing for this POST (see `answer-over-a-stale-connection.spec.ts`).
test.use({ serviceWorkers: 'block' });

/** A Claude Code thread parked on a question card, single-select unless
 *  `multiSelect`, seeded straight into the database. The caller cleans it up
 *  with `cleanupCCThread`. */
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

/** Hold `button` long enough to turn on side-question mode. A mouse hold,
 *  which every project accepts. */
async function holdButton(page: Page, button: Locator): Promise<void> {
  const box = (await button.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
}

/** The round button that asks the box while side-question mode is on. */
const roundAsk = (page: Page): Locator =>
  page.locator('button.send-cancel-round[aria-label="Ask side question"]:visible').first();

/** Ask `question` as a side question: type it, hold Send to turn on the mode,
 *  then press the round Ask. */
async function askBySendHold(page: Page, question: string): Promise<void> {
  const input = await waitForVisibleInput(page, 15_000);
  await input.fill(question);
  await holdButton(page, page.locator('[aria-label="Send message"]:visible').first());
  await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
  await roundAsk(page).click();
}

/** A side question in a Claude Code thread (ADR 0320). The question goes to the
 *  side-question endpoint, the answer lands on a card that survives a reload,
 *  and no turn of the thread shows it. Runs on desktop and on the mobile
 *  projects, since the card must work on the iOS PWA too.
 *
 *  It runs against the real Claude Code: a copy of the session that persists
 *  nothing (ADR 0324). */
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

    await askBySendHold(page, 'What codeword did you just say? Reply with the codeword only.');

    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card).toBeVisible();
    await expect(card.locator('.side-question-question')).toHaveText(
      'What codeword did you just say? Reply with the codeword only.',
    );
    await expect(card).toHaveAttribute('data-status', 'answered', { timeout: 150_000 });
    await expect(card.locator('.side-question-body > .markdown-content')).toContainText(token);
    await expect(card).toContainText('Not added to the conversation');

    // The side Q&A is not a turn: no new exchange, no user message, no chat POST.
    expect(await countExchanges(page)).toBe(exchangesBefore);
    expect(chatPosts.length).toBe(postsBefore);

    // The whole head is the toggle. Folding rolls the body away and leaves
    // the card in place as one line.
    const head = card.locator('button.side-question-head');
    await expect(head).toHaveAttribute('aria-expanded', 'true');
    // The head is shorter than a finger, so its hit area reaches past its box.
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
    await expect(card.locator('.side-question-summary')).toContainText('What codeword did you just say?');
    await expect(card.locator('.side-question-question')).toHaveCount(0);

    // Recorded as events, so the card is still folded after a reload, and a
    // tap on its head opens the answer again.
    await page.reload();
    const folded = page.locator('[data-role="side-question-card"][data-collapsed]:visible').first();
    await expect(folded).toBeVisible({ timeout: 30_000 });
    await folded.locator('button.side-question-head').click();
    await expect(page.locator('[data-role="side-question-card"]:visible .side-question-body > .markdown-content')).toContainText(token);
    expect(await countExchanges(page)).toBe(exchangesBefore);
  });

  // While a question card waits, a typed draft turns the row's end button into
  // Submit. Its hold turns on side-question mode with the draft kept, and the
  // round Ask then asks it aside, never as an answer to the card.
  test('holding Submit turns on the mode and the round Ask leaves the card waiting', async ({ page }) => {
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
      await holdButton(page, submit);

      await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
      await expect(input).toHaveValue('what does option two mean?');
      // One Ask button in every state: the round one, never a square "Ask".
      await expect(roundAsk(page)).toBeVisible();
      await expect(page.locator('button[aria-label="Submit answer"]:visible')).toHaveCount(0);
      // The hold's own release answered nothing.
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

  // An empty box on a waiting card shows a lone Cancel. Its hold turns on
  // side-question mode, as Stop's does, and the release never cancels. The
  // mode then wins over the card: a typed box shows the round Ask, and asking
  // leaves the card waiting.
  test('holding Cancel on a waiting card starts a side question', async ({ page }) => {
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
      // The row click slides the pane in, and what follows reads geometry.
      await waitForPaneAtRest(page);
      const card = page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first();
      await expect(card).toBeVisible({ timeout: 15_000 });

      const input = await waitForVisibleInput(page, 15_000);
      await expect(input).toHaveValue('');
      const cancel = page.locator('button.action-btn-danger[aria-label="Cancel"]:not(:disabled):visible').first();
      await expect(cancel).toBeVisible({ timeout: 10_000 });
      const box = (await cancel.boundingBox())!;
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.down();
      await page.waitForTimeout(700);
      await page.mouse.up();

      await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
      await expect(input).toHaveAttribute('placeholder', 'Ask a side question…');
      expect(answerOrStopPosts, 'the hold\'s release cancelled nothing').toEqual([]);

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

  // Before any pick, a multi-select card's Submit reads disabled but still
  // takes the hold, which turns on the mode and never answers.
  test('holding a multi-select card\'s Submit turns on the mode, even before a pick', async ({ page }) => {
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

      const multiSubmit = page.locator('button.split-button-primary[aria-label="Submit answer"]:visible');
      await expect(multiSubmit).toHaveAttribute('aria-disabled', 'true', { timeout: 10_000 });
      await holdButton(page, multiSubmit);
      await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
      await expect(multiSubmit).toHaveCount(0);
      expect(answerPosts).toEqual([]);
      await expect(card).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  // A multi-select card had no way in. The shortcut now turns on the mode
  // there too. The card's split button steps aside, so the box asks.
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

      const multiSubmit = page.locator('button.split-button-primary[aria-label="Submit answer"]:visible');
      await expect(multiSubmit).toBeVisible({ timeout: 10_000 });
      const input = await waitForVisibleInput(page, 15_000);
      await input.focus();
      await page.keyboard.press('Alt+Enter');
      await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
      await expect(multiSubmit).toHaveCount(0);
      await page.keyboard.type('what does option two mean?');
      await expect(roundAsk(page)).toBeVisible();
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

    await askBySendHold(page, 'anything');
    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card).toHaveAttribute('data-status', 'failed');
    await expect(card.getByRole('alert')).toHaveText('Side questions are not available in Codex threads.');
  });
});

/** A Lucidos Agent thread answers from its own model (the mock, in e2e). Every
 *  hold turns on side-question mode, with any draft kept in the box. The
 *  release that ends a hold never also acts. */
test.describe('side questions in a Lucidos Agent thread', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('holding Send turns on the mode with the draft kept, and Ask asks it', async ({ page }) => {
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
    await holdButton(page, page.locator('[aria-label="Send message"]:visible').first());

    await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
    await expect(input).toHaveValue('what did you just say?');
    // The hold's own release sent nothing.
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

  // A mouse hold hands focus back to the composer, so the user types on and
  // Enter asks the whole draft rather than sending it.
  test('typing on after a hold, then Enter, asks the whole draft', async ({ page }) => {
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
    await holdButton(page, page.locator('[aria-label="Send message"]:visible').first());

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

  // An idle thread with an empty box had no way in. The shortcut now turns
  // the mode on there, and a second press turns it off with the text kept.
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

  // Stop shows only over an empty box, and its hold turns on side-question
  // mode. The release that ends the hold does not also stop the turn. The mode survives a reload, and its × leaves it with
  // the text kept.
  test('holding Stop turns on side-question mode and leaves the turn running', async ({ page }) => {
    const cancelPosts: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && /\/api\/v1\/chat\/cancel\b/.test(req.url())) cancelPosts.push(req.url());
    });

    await navigateToApp(page);
    await newThread(page);
    await sendMessage(page, `Write an extremely long and detailed essay about the history of bridges. Be as verbose as possible. Include: ${uniqueMessage('stop-hold')}`);
    await waitForStreamingToStart(page, 5, 60_000);

    // Past the settle window, which holds a fresh Stop disabled.
    const stop = page.locator('button.send-cancel-morph[aria-label="Cancel"]:not(:disabled):visible').first();
    await expect(stop).toBeVisible({ timeout: 30_000 });
    const box = (await stop.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.waitForTimeout(700);
    await page.mouse.up();

    const input = await waitForVisibleInput(page, 15_000);
    const pill = page.locator('[data-role="side-question-mode"]:visible');
    await expect(pill).toBeVisible();
    await expect(input).toHaveValue('');
    await expect(input).toHaveAttribute('placeholder', 'Ask a side question…');
    await expect(input).toBeFocused();
    // Stop stays until the box holds text, so the turn can still be stopped.
    await expect(stop).toBeVisible();

    // The PUT that carries the typing carries the mode too.
    const saved = page.waitForResponse((res) => res.url().includes('/compose')
      && res.request().method() === 'PUT' && (res.request().postData() ?? '').includes('"sideQuestionMode":true'));
    await page.keyboard.type('what');
    await expect(input).toHaveValue('what');
    expect((await saved).ok()).toBe(true);

    await page.reload();
    const reloaded = await waitForVisibleInput(page, 30_000);
    await expect(pill).toBeVisible();
    await expect(reloaded).toHaveValue('what');

    // Off is stored too, so a reload does not bring the pill back.
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

  // Over an empty box the shortcut turns on side-question mode too, so the box
  // takes typing at once and Enter asks. Escape leaves the mode before it
  // would stop the turn.
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

    // Escape leaves the mode and keeps the text. The turn keeps running.
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
});
