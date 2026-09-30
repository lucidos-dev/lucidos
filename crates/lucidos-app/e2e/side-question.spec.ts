import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage, assertHealthy,
  pickComposeDestination, newThread, countExchanges, waitForActionPanel,
  waitForResponse, waitForVisibleInput, waitForStreamingToStart, isMobileViewport,
} from './helpers';

// A controlled page sends fetches through the service worker in WebKit, where
// `page.route` cannot see them. `sw.js` handles GETs only, so blocking it
// changes nothing for this POST (see `answer-over-a-stale-connection.spec.ts`).
test.use({ serviceWorkers: 'block' });

/** `/btw` in a Claude Code thread (ADR 0320). The question goes to the
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

  test('/btw answers on a card and never enters the thread', async ({ page }) => {
    const chatPostsWithBtw: string[] = [];
    page.on('request', (req) => {
      if (req.method() === 'POST' && /\/api\/v1\/chat\b/.test(req.url()) && (req.postData() ?? '').includes('/btw')) {
        chatPostsWithBtw.push(req.url());
      }
    });

    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);
    const token = uniqueMessage('btw-codeword');
    await sendMessage(page, `Say exactly: "codeword ${token}" and nothing else. Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);
    const exchangesBefore = await countExchanges(page);

    await sendFollowUp(page, '/btw What codeword did you just say? Reply with the codeword only.');

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
    await expect(page.locator('.chat-exchange:visible', { hasText: '/btw' })).toHaveCount(0);
    expect(chatPostsWithBtw).toEqual([]);

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

  test('a failed side question says why on the card', async ({ page }) => {
    await page.route('**/api/v1/side-questions', (route) => route.fulfill({
      status: 400,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'Side questions are not available in Codex threads.' }),
    }));
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);
    await sendMessage(page, `Say exactly: "${uniqueMessage('btw-refusal')}". Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);

    await sendFollowUp(page, '/btw anything');
    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card).toHaveAttribute('data-status', 'failed');
    await expect(card.getByRole('alert')).toHaveText('Side questions are not available in Codex threads.');
  });
});

/** A Lucidos Agent thread answers from its own model (the mock, in e2e). A
 *  hold on Send asks the draft as a side question, with no `/btw` typed, and a
 *  hold on Stop starts one. The release that ends a hold never also acts. */
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

  // Stop shows only over an empty box, so its hold starts a `/btw` draft to
  // finish. The release that ends the hold does not also stop the turn.
  test('holding Stop starts a side question and leaves the turn running', async ({ page }) => {
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

    const ask = page.locator('[data-role="ask-side-question"]:visible');
    await expect(ask).toBeVisible();
    await ask.click();

    const input = await waitForVisibleInput(page, 15_000);
    await expect(input).toHaveValue('/btw ');
    await expect(input).toBeFocused();
    expect(cancelPosts).toEqual([]);
  });
});
