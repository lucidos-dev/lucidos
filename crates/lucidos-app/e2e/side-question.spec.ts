import type { Page } from '@playwright/test';
import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, uniqueMessage, assertHealthy,
  pickComposeDestination, newThread, countExchanges, waitForActionPanel,
  waitForResponse, waitForVisibleInput, waitForStreamingToStart, isMobileViewport,
  openThreadDrawer, ensureOnThreadPane,
} from './helpers';
import { psql, createCCThreadWithChange, cleanupCCThread } from './db-helpers';

// A controlled page sends fetches through the service worker in WebKit, where
// `page.route` cannot see them. `sw.js` handles GETs only, so blocking it
// changes nothing for this POST (see `answer-over-a-stale-connection.spec.ts`).
test.use({ serviceWorkers: 'block' });

/** A Claude Code thread parked on a single-select question card, seeded
 *  straight into the database. The caller cleans it up with `cleanupCCThread`. */
function seedWaitingCard(name: string) {
  const suffix = randomUUID().slice(0, 8);
  const toolUseId = `tu-side-${suffix}`;
  const { threadId, changeId, branch, file } = createCCThreadWithChange(name, suffix);
  const question = JSON.stringify({
    tool_use_id: toolUseId,
    cc_session_id: '',
    channel: 'claude_code',
    multi_select: false,
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

/** Ask `question` as a side question: type it, hold Send, press the pill's
 *  Side question half. A mouse hold, which every project accepts. */
async function askBySendHold(page: Page, question: string): Promise<void> {
  const input = await waitForVisibleInput(page, 15_000);
  await input.fill(question);
  const send = page.locator('[aria-label="Send message"]:visible').first();
  const box = (await send.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
  await page.locator('[data-role="ask-side-question"]:visible').click();
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
    await expect(card.locator('.markdown-content')).toContainText(token);
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
    await expect(page.locator('[data-role="side-question-card"]:visible .markdown-content')).toContainText(token);
    expect(await countExchanges(page)).toBe(exchangesBefore);
  });

  // While a question card waits, a typed draft turns the row's end button into
  // Submit. The pill slides out of Submit as it does out of Send. Asking from
  // it sends a side question, never an answer to the card.
  test('holding Submit asks the draft aside and leaves the card waiting', async ({ page }) => {
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
      await expect(page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first())
        .toBeVisible({ timeout: 15_000 });

      const input = await waitForVisibleInput(page, 15_000);
      await input.fill('what does option two mean?');
      const submit = page.locator('button[aria-label="Submit answer"]:visible').first();
      await expect(submit).toBeVisible({ timeout: 10_000 });
      const box = (await submit.boundingBox())!;
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.down();
      await page.waitForTimeout(700);
      await page.mouse.up();

      const ask = page.locator('[data-role="ask-side-question"]:visible');
      await expect(ask).toBeVisible();
      await expect(submit).toHaveClass(/\bsplit-open\b/);
      // Past the slide, so the half sits at rest against the seam.
      await page.waitForTimeout(400);
      const half = (await ask.boundingBox())!;
      const button = (await submit.boundingBox())!;
      expect(Math.abs(half.x + half.width - button.x), 'the half ends at Submit\'s left edge').toBeLessThan(1);
      expect(Math.abs(half.height - button.height), 'the half is as tall as Submit').toBeLessThan(1);
      expect(Math.abs(half.y - button.y)).toBeLessThan(1);
      // Opening keeps Submit's width, so the row's fold has nothing to redo.
      expect(Math.abs(button.width - box.width), 'Submit kept its width').toBeLessThan(0.5);
      // Nothing in the row draws over the half.
      const centre = { x: half.x + half.width / 2, y: half.y + half.height / 2 };
      const onTop = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)
        ?.closest('[data-role="ask-side-question"]') !== null, centre);
      expect(onTop).toBe(true);
      // The hold's own release answered nothing.
      expect(answerPosts).toEqual([]);

      await ask.click();
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
  // mode then wins over the card: the button reads Ask, and asking leaves the
  // card waiting.
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
      await page.locator('button[aria-label="Ask side question"]:visible').first().click();
      await expect.poll(() => sideQuestionBodies.length).toBe(1);
      expect(sideQuestionBodies[0]).toContain('what does option one mean?');
      expect(answerOrStopPosts).toEqual([]);
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

/** A Lucidos Agent thread answers from its own model (the mock, in e2e). A
 *  hold on Send asks the draft as a side question, and a mouse hold on Stop
 *  turns on side-question mode. The release that ends a hold never also acts. */
test.describe('side questions in a Lucidos Agent thread', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('holding Send asks the draft as a side question', async ({ page }) => {
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
    const send = page.locator('[aria-label="Send message"]:visible').first();
    const box = (await send.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.waitForTimeout(700);
    await page.mouse.up();

    const ask = page.locator('[data-role="ask-side-question"]:visible');
    await expect(ask).toBeVisible();
    // The hold's own release sent nothing.
    expect(chatPosts.length).toBe(postsBefore);
    await ask.click();

    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card.locator('.side-question-question')).toHaveText('what did you just say?');
    await expect(card).toHaveAttribute('data-status', 'answered', { timeout: 60_000 });
    await expect(input).toHaveValue('');
    expect(await countExchanges(page)).toBe(exchangesBefore);
    expect(chatPosts.length).toBe(postsBefore);
  });

  // The pill keeps the draft editable: a mouse hold hands focus back to the
  // composer, and Enter there asks the side question rather than sending.
  test('typing on while the pill is open, then Enter, asks the whole draft', async ({ page }) => {
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
    const send = page.locator('[aria-label="Send message"]:visible').first();
    const box = (await send.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.waitForTimeout(700);
    await page.mouse.up();

    await expect(page.locator('[data-role="ask-side-question"]:visible')).toBeVisible();
    await expect(input).toBeFocused();
    await page.keyboard.type(' you just say?');
    await expect(input).toHaveValue('what did you just say?');
    await page.keyboard.press('Enter');

    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card.locator('.side-question-question')).toHaveText('what did you just say?');
    await expect(page.locator('[data-role="ask-side-question"]:visible')).toHaveCount(0);
    await expect(input).toHaveValue('');
    expect(await countExchanges(page)).toBe(exchangesBefore);
    expect(chatPosts.length).toBe(postsBefore);
  });

  // The shortcut toggles: a second press shuts the pill as Escape does, and
  // hands focus back to the draft.
  test('pressing the Side question shortcut again shuts the pill', async ({ page }) => {
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
    await input.fill('what did you just say?');
    await input.focus();
    const ask = page.locator('[data-role="ask-side-question"]:visible');
    await page.keyboard.press('Alt+Enter');
    await expect(ask).toBeVisible();
    await page.keyboard.press('Alt+Enter');
    await expect(ask).toHaveCount(0);
    await expect(input).toBeFocused();
    await expect(input).toHaveValue('what did you just say?');
    expect(sideQuestionPosts).toEqual([]);
  });

  // Stop shows only over an empty box, so its hold turns on side-question
  // mode, with no pill to press first. The release that ends the hold does not
  // also stop the turn. The mode survives a reload, and its × leaves it with
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
    await expect(page.locator('[data-role="ask-side-question"]:visible')).toHaveCount(0);

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
    await expect(page.locator('[data-role="ask-side-question"]:visible')).toHaveCount(0);

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
