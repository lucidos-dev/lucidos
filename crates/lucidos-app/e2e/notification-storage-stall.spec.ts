import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { randomUUID } from 'crypto';
import { assertHealthy, gotoWithRetry, sendMessage } from './helpers';
import { psql } from './db-helpers';

/** A notification cold open while the restore-generation database hangs.
 *  Restore admission waits on that database, but a provisional read shows the
 *  target thread before admission (ADR 0704). Sends still wait for admission. */

type Wedge = 'first-connection' | 'every-connection';

/** The owner's bound for a notification tap, measured from navigation start. */
const TARGET_SHOWN_WITHIN_MS = 2_000;

/** Runs before the app: a wedged connection takes transactions that never
 *  settle, apply nothing and ignore abort, as WebKit's can after a resume. */
function wedgeRestoreStorage(mode: Wedge) {
  const state = { mode, first: null as IDBDatabase | null };
  (window as unknown as { restoreStorageWedge: typeof state }).restoreStorageWedge = state;
  const transaction = IDBDatabase.prototype.transaction;
  IDBDatabase.prototype.transaction = function (this: IDBDatabase, ...args: Parameters<typeof transaction>) {
    if (this.name.startsWith('lucidos-restore-generation')) {
      state.first ??= this;
      if (state.mode === 'every-connection' || (state.mode === 'first-connection' && this === state.first)) {
        const pending = {};
        return { objectStore: () => ({ get: () => pending, put: () => {} }), abort: () => {} } as unknown as IDBTransaction;
      }
    }
    return transaction.apply(this, args);
  };
}

function seedThread(text: string): string {
  const threadId = randomUUID();
  const now = Date.now();
  const at = (k: number) => new Date(now - 60_000 + k * 1000).toISOString();
  const row = (type: string, payload: unknown, created: string) =>
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', '${type}', '${JSON.stringify(payload).replace(/'/g, "''")}'::jsonb, '${created}', 'thread', '${threadId}', '${threadId}')`;
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Storage stall target', 'chat', '${at(2)}', 1, false, true, 'completed', 'inbox', 'active', false, 0, 0, 'none', false)`,
    row('MessageReceived', { text, channel: 'chat' }, at(0)),
    row('ResponseGenerated', { text: 'Seeded response.', images: [] }, at(1)),
  ].join(';\n'));
  return threadId;
}

function messageCount(threadId: string): number {
  return Number(psql(`SELECT count(*) FROM events WHERE aggregate_id = '${threadId}' AND event_type = 'MessageReceived'`));
}

/** The URL an iOS notification tap opens: a query string, never a hash. The
 *  row exists so marking it read after admission succeeds. */
function notificationTapUrl(threadId: string): { url: string; notificationId: string } {
  const notificationId = randomUUID();
  const tap = JSON.stringify({ kind: 'navigate', to: { target: 'thread', id: threadId } });
  psql(`INSERT INTO notifications (id, thread_id, title, message, tap) VALUES ('${notificationId}', '${threadId}', 'Storage stall target', 'tap me', '${tap}')`);
  const query = new URLSearchParams({ notification: notificationId, thread: threadId, tap });
  return { url: `/?${query}`, notificationId };
}

/** Milliseconds from navigation start until a visible element shows `text`. */
async function shownAtMs(page: Page, text: string): Promise<number> {
  const shownAt = await page.waitForFunction((needle) => {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.textContent?.includes(needle) && node.parentElement?.checkVisibility()) return performance.now();
    }
    return null;
  }, text, { polling: 'raf', timeout: 30_000 });
  return (await shownAt.jsonValue()) as number;
}

test.describe('A notification cold open while device storage hangs', () => {
  let threadId: string | null = null;
  let notificationId: string | null = null;

  test.afterEach(() => {
    if (notificationId) psql(`DELETE FROM notifications WHERE id = '${notificationId}'`);
    notificationId = null;
    if (!threadId) return;
    psql([
      `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
      `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
    ].join(';\n'));
    threadId = null;
  });

  test('reaches its target on a fresh database connection', async ({ page }) => {
    await assertHealthy(page);
    const text = `Storage stall target ${randomUUID()}`;
    threadId = seedThread(text);
    await page.addInitScript(wedgeRestoreStorage, 'first-connection' as Wedge);
    await gotoWithRetry(page, `/#thread=${threadId}`);
    await expect(page.getByText(text).first()).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('.toast', { hasText: 'Workspace recovery' })).toHaveCount(0);
    await expect(page.locator('.toast', { hasText: 'Device storage' })).toHaveCount(0);
  });

  test('shows its target at once, holds a send while storage hangs, and sends once it answers', async ({ page }) => {
    // The storage card takes most of a minute to appear.
    test.setTimeout(180_000);
    await assertHealthy(page);
    const text = `Storage stall target ${randomUUID()}`;
    threadId = seedThread(text);
    const tap = notificationTapUrl(threadId);
    notificationId = tap.notificationId;
    const sends: string[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && request.url().includes('/api/v1/chat/stream')) sends.push(request.url());
    });
    await page.addInitScript(wedgeRestoreStorage, 'every-connection' as Wedge);

    await gotoWithRetry(page, tap.url);
    expect(await shownAtMs(page, text)).toBeLessThan(TARGET_SHOWN_WITHIN_MS);

    const reply = `Held reply ${randomUUID()}`;
    await sendMessage(page, reply);
    const warning = page.locator('.toast', { hasText: 'Device storage' });
    await expect(warning).toBeVisible({ timeout: 60_000 });
    await expect(warning).toContainText('You can read, but sending waits');
    await expect(warning).not.toContainText('lucidos-restore-generation');
    expect(sends).toEqual([]);
    expect(messageCount(threadId)).toBe(1);

    await page.evaluate(() => {
      (window as unknown as { restoreStorageWedge: { mode: string } }).restoreStorageWedge.mode = 'none';
    });
    // A lone neutral action makes the whole card its tap.
    await warning.click();
    await expect(warning).toHaveCount(0);
    // The held send outlived its own deadline, yet goes out by itself.
    await expect.poll(() => messageCount(threadId!), { timeout: 30_000 }).toBe(2);
    await expect(page.locator('.exchange-error', { hasText: 'Not sent' })).toHaveCount(0);
  });
});
