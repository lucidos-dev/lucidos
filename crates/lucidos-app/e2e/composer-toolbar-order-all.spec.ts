import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, waitForVisibleInput } from './helpers';
import { psql, seedChatThread } from './db-helpers';

test('composer icons and right-aligned controls stay put when clear draft toggles', async ({ page }) => {
  const threadId = seedChatThread({ turns: 1, needles: [], title: 'Composer toolbar order' });
  psql(`INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id)
    VALUES (gen_random_uuid(), 'TodoListWritten',
      '{"items":[{"content":"Check toolbar","active_form":"Checking toolbar","status":"pending"}]}',
      NOW(), 'thread', '${threadId}', '${threadId}')`);
  try {
    await page.addInitScript((id) => {
      localStorage.setItem('lucidos-focused-thread', id);
      localStorage.setItem('lucidos-split-ratio', '1');
    }, threadId);
    await assertHealthy(page);
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);
    const row = page.locator('.prompt-actions-row:visible');
    const icons = [
      '[data-role="side-question-toggle"]', '[data-role="attach-image"]', '[data-role="todo-indicator"]',
    ];
    const controls = [...icons, '.rung-control', '.action-slot'];
    for (const selector of controls) await expect(row.locator(selector)).toBeVisible();
    // The follow toggle floats over the composer, so it takes no box here.
    await expect(row.locator('[data-role="follow-live-edge"]')).toHaveCount(0);
    const positions = () => Promise.all(controls.map((selector) => row.locator(selector).boundingBox()));
    const empty = await positions();
    for (let i = 1; i < empty.length; i++) {
      expect(empty[i]!.x).toBeGreaterThanOrEqual(empty[i - 1]!.x + empty[i - 1]!.width);
    }
    await expect(row.locator('.prompt-clear')).toHaveCount(0);

    await input.fill('Draft text');
    const clear = row.locator('.prompt-clear');
    await expect(clear).toBeVisible();
    const clearBox = (await clear.boundingBox())!;
    expect(clearBox.x).toBeGreaterThanOrEqual(empty[2]!.x + empty[2]!.width);
    expect(clearBox.x + clearBox.width).toBeLessThanOrEqual(empty[3]!.x);
    await expect.poll(positions).toEqual(empty);

    await clear.click();
    await expect(input).toHaveValue('');
    await expect(clear).toHaveCount(0);
    await expect.poll(positions).toEqual(empty);
  } finally {
    psql(`DELETE FROM events WHERE thread_id = '${threadId}';
      DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`);
  }
});
