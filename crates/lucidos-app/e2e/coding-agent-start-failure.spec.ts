import { fileURLToPath } from 'node:url';
import { test, expect } from './fixtures';
import {
  apiRequest, assertHealthy, navigateToApp, newThread, pickComposeDestination, sendMessage,
} from './helpers';
import { clearAllThreads, psql } from './db-helpers';

const FAKE_CLAUDE = fileURLToPath(new URL('./fake-claude-code/claude.mjs', import.meta.url));
const GATEWAY_WORDS = 'Could not load settings from claude-gateway.example.com. Check your network connection.';
const PREF = 'coding_agent_claude_path';

function testThreadId(): string {
  return psql('SELECT thread_id FROM thread_summaries WHERE NOT is_home ORDER BY created_at DESC LIMIT 1');
}

function eventCount(threadId: string, eventType: string): number {
  return Number(psql(`SELECT count(*) FROM events WHERE thread_id = '${threadId}' AND event_type = '${eventType}'`));
}

test.describe('a coding agent that cannot start', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
    const res = await apiRequest(page).put(`/api/v1/preferences?key=${PREF}`, { data: { value: FAKE_CLAUDE } });
    expect(res.ok(), `${PREF}=${FAKE_CLAUDE}`).toBe(true);
  });

  test.afterEach(() => {
    psql(`DELETE FROM preferences WHERE key = '${PREF}'`);
  });

  test('shows the CLI words and Try again, retries nothing by itself, and resends the message on Try again', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);
    await sendMessage(page, `Fail to start: "${GATEWAY_WORDS}"`);

    const card = page.locator('.exchange-error:visible').first();
    await expect(card.locator('strong')).toHaveText('Claude Code could not start', { timeout: 60_000 });
    await expect(card.locator('.exchange-error-output')).toHaveText(GATEWAY_WORDS);
    await expect(card.locator('.exchange-error-reason')).toContainText('Lucidos did not retry');
    const tryAgain = card.getByRole('button', { name: 'Try again' });
    await expect(tryAgain).toBeVisible();

    const threadId = testThreadId();
    expect(psql(`SELECT status FROM thread_summaries WHERE thread_id = '${threadId}'`)).toBe('failed');
    expect(eventCount(threadId, 'ResponseFailed')).toBe(1);
    await page.waitForTimeout(3_000);
    expect(eventCount(threadId, 'ContinuationRequested')).toBe(0);
    expect(eventCount(threadId, 'ResponseFailed')).toBe(1);

    await tryAgain.click();

    await expect(page.locator('.exchange-error-output', { hasText: GATEWAY_WORDS })).toHaveCount(2, { timeout: 60_000 });
    expect(eventCount(threadId, 'ContinuationRequested')).toBe(1);
    expect(eventCount(threadId, 'ResponseFailed')).toBe(2);
  });
});
