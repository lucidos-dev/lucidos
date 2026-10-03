import { test, expect, type Page } from './fixtures';
import type { Route } from '@playwright/test';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  uniqueMessage,
  USER_MSG_SELECTOR,
  waitForResponse,
  waitForVisibleInput,
} from './helpers';

/** A send that got no answer, or that a reload cut off, comes back after the
 *  reload as a Not sent card with Retry. Exercises the real IndexedDB store,
 *  which Vitest cannot reach.
 *  Plan: `docs/plans/2026-10-03-unsent-messages-survive-a-reload.md`.
 *
 *  Service workers are blocked so `page.route` sees the chat POST in WebKit. */

test.use({ serviceWorkers: 'block' });

const CHAT_ROUTE = '**/api/v1/chat/stream';

const notSentCard = (page: Page) => page.locator('.exchange-error:visible', { hasText: 'Not sent' });
const userBubble = (page: Page, text: string) => page.locator(`${USER_MSG_SELECTOR}:visible`, { hasText: text });

/** How many sends the unsent-message database holds. */
function storedCount(page: Page): Promise<number> {
  return page.evaluate(async () => {
    let count = 0;
    for (const { name } of await indexedDB.databases()) {
      if (!name?.startsWith('lucidos-unsent-messages')) continue;
      count += await new Promise<number>((resolve, reject) => {
        const open = indexedDB.open(name);
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          const db = open.result;
          const req = db.transaction('messages', 'readonly').objectStore('messages').count();
          req.onsuccess = () => { db.close(); resolve(req.result); };
          req.onerror = () => { db.close(); reject(req.error); };
        };
      });
    }
    return count;
  });
}

/** Type a draft and wait for the engine to store it, as a real first send has. */
async function typeStoredDraft(page: Page, message: string): Promise<void> {
  const stored = page.waitForResponse((r) =>
    r.url().endsWith('/compose') && r.request().method() === 'PUT' && r.ok());
  const input = await waitForVisibleInput(page);
  await input.fill(message);
  await stored;
}

async function pressSend(page: Page): Promise<void> {
  await page.locator('button[aria-label="Send message"]:visible').first().click();
}

/** Hold every chat POST until the page goes away. */
async function holdChatPosts(page: Page): Promise<void> {
  await page.route(CHAT_ROUTE, () => new Promise<void>(() => {}));
}

test.describe('An unsent message survives a page reload', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a first send that got no answer comes back with Retry, and Retry sends it once', async ({ page }) => {
    await navigateToApp(page);
    const message = uniqueMessage('unsent-first');
    await typeStoredDraft(page, message);
    await page.route(CHAT_ROUTE, (route: Route) => route.abort('failed'));
    await pressSend(page);
    await expect(notSentCard(page)).toHaveCount(1);
    await expect.poll(() => storedCount(page)).toBe(1);

    await page.unroute(CHAT_ROUTE);
    await page.reload();
    await expect(notSentCard(page)).toHaveCount(1, { timeout: 15_000 });
    await expect(userBubble(page, message)).toHaveCount(1);
    // The draft it was written from is not back in the composer beside it.
    await expect(await waitForVisibleInput(page)).toHaveValue('');

    await notSentCard(page).getByRole('button', { name: 'Retry' }).click();
    await expect(notSentCard(page)).toHaveCount(0);
    await waitForResponse(page);
    await expect(userBubble(page, message)).toHaveCount(1);
    await expect.poll(() => storedCount(page)).toBe(0);
  });

  test('a follow-up the reload cut off comes back, and Discard drops it for good', async ({ page }) => {
    await navigateToApp(page);
    const first = uniqueMessage('unsent-follow-up-first');
    await sendMessage(page, `Say exactly: "${first}"`);
    await waitForResponse(page);

    await holdChatPosts(page);
    const followUp = uniqueMessage('unsent-follow-up');
    await sendMessage(page, followUp);
    await expect.poll(() => storedCount(page)).toBe(1);

    await page.unroute(CHAT_ROUTE);
    await page.reload();
    await expect(notSentCard(page)).toHaveCount(1, { timeout: 15_000 });
    // Unloading may reject the held POST first, and the page then records "no
    // answer" before it goes. Either way the message must come back.
    await expect(notSentCard(page)).toContainText(/reloaded before Lucidos answered|did not answer/);
    await expect(userBubble(page, followUp)).toHaveCount(1);

    await notSentCard(page).getByRole('button', { name: 'Discard' }).click();
    await expect(notSentCard(page)).toHaveCount(0);
    await expect(userBubble(page, followUp)).toHaveCount(0);
    await expect.poll(() => storedCount(page)).toBe(0);

    await page.reload();
    await expect(userBubble(page, first)).toHaveCount(1, { timeout: 15_000 });
    await expect(notSentCard(page)).toHaveCount(0);
  });
});
