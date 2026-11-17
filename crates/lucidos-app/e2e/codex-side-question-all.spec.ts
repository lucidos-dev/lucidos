import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import {
  navigateToApp, openThreadDrawer, ensureOnThreadPane, waitForPaneAtRest,
  waitForVisibleInput, composerControl,
} from './helpers';
import { psql, createCCThreadWithChange, cleanupCCThread } from './db-helpers';

// WebKit's service worker hides intercepted POSTs from page.route.
test.use({ serviceWorkers: 'block' });

test('Codex exposes side-question mode and routes an ask to its own card', async ({ page }) => {
  const suffix = randomUUID().slice(0, 8);
  const title = `E2E Codex Side ${suffix}`;
  const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Codex Side', suffix);
  let asks = 0;
  try {
    psql(`UPDATE thread_summaries SET coding_agent = 'codex' WHERE thread_id = '${threadId}'`);
    await page.route('**/api/v1/side-questions', (route) => {
      asks += 1;
      return route.fulfill({
        status: 400, contentType: 'application/json',
        body: JSON.stringify({ error: 'Update Codex to use ephemeral side-question forks.' }),
      });
    });
    await navigateToApp(page);
    await openThreadDrawer(page);
    await page.locator(`.thread-row:has-text("${title}")`).first().click();
    await ensureOnThreadPane(page);
    await waitForPaneAtRest(page);
    await (await waitForVisibleInput(page, 15_000)).fill('What does the current step mean?');
    await (await composerControl(page, '[data-role="side-question-toggle"]')).click();
    await expect(page.locator('[data-role="side-question-mode"]:visible')).toBeVisible();
    await page.locator('button.send-cancel-morph[aria-label="Ask side question"]:visible').first().click();
    const card = page.locator('[data-role="side-question-card"]:visible').first();
    await expect(card).toHaveAttribute('data-status', 'failed');
    await expect(card.getByRole('alert')).toHaveText('Update Codex to use ephemeral side-question forks.');
    expect(asks).toBe(1);
  } finally {
    cleanupCCThread(threadId, changeId, branch, file);
  }
});
