import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage, assertHealthy,
  pickComposeDestination, newThread, countExchanges, waitForActionPanel,
} from './helpers';

// A controlled page sends fetches through the service worker in WebKit, where
// `page.route` cannot see them. `sw.js` handles GETs only, so blocking it
// changes nothing for this POST (see `answer-over-a-stale-connection.spec.ts`).
test.use({ serviceWorkers: 'block' });

/** `/btw` in a Claude Code thread (ADR 0318). The question goes to the
 *  side-question endpoint, the answer lands on a dismissible card, and the
 *  thread's own history is untouched. Runs on desktop and on the mobile
 *  projects, since the card must work on the iOS PWA too.
 *
 *  The thread is idle when asked, so this crosses the cold path against the
 *  real Claude Code: a resumed process that persists nothing. */
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

    const dismiss = card.getByRole('button', { name: 'Dismiss side question' });
    const box = await dismiss.boundingBox();
    expect(box!.width).toBeGreaterThanOrEqual(44);
    expect(box!.height).toBeGreaterThanOrEqual(44);
    await dismiss.click();
    await expect(page.locator('[data-role="side-question-card"]:visible')).toHaveCount(0);
  });

  test('a failed side question says why on the card', async ({ page }) => {
    await page.route('**/api/v1/coding-agents/side-question', (route) => route.fulfill({
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
