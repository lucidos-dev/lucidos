import { test, expect, Page, Locator } from './fixtures';
import { addTriggerCard, apiRequest, assertHealthy, navigateToApp, openTriggersPanel } from './helpers';

function visibleForm(page: Page): Locator {
  return page.locator('.inline-form:visible').first();
}

test.describe('Trigger form — Program grants', () => {
  let triggerName: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    triggerName = `e2e-program-grants-${Date.now()}`;
    await apiRequest(page).put('/api/v1/preferences?key=timezone', { data: { value: 'UTC' } });
  });

  test.afterEach(async ({ page }) => {
    try {
      const res = await page.request.get('/api/v1/triggers');
      const body = await res.json();
      const mine = (body.triggers ?? []).find((t: { name: string; id: string }) => t.name === triggerName);
      if (mine) await apiRequest(page).delete(`/api/v1/triggers?id=${mine.id}`);
    } catch {
    }
  });

  test('a new trigger shows an empty program grant list', async ({ page }) => {
    await navigateToApp(page);
    await openTriggersPanel(page);

    await addTriggerCard(page).click();
    const form = visibleForm(page);
    await expect(form).toBeVisible({ timeout: 10_000 });

    await form.locator('input[placeholder="e.g. Morning Brief"]').fill(triggerName);
    await form.locator('input[placeholder="0 0 8 * * *"]').fill('0 0 8 * * *');
    await form.locator('.prompt-textarea').fill('Send me a hello every morning');

    await form.locator('.btn-save').click();
    await expect(page.locator('.trigger-row .list-row-name', { hasText: triggerName }).first())
      .toBeVisible({ timeout: 10_000 });

    await navigateToApp(page);
    await openTriggersPanel(page);
    await page.locator('.trigger-row .list-row-name', { hasText: triggerName }).first().click();

    const editForm = visibleForm(page);
    await expect(editForm).toBeVisible({ timeout: 10_000 });
    await expect(editForm.getByText('Program grants', { exact: true })).toBeVisible();
    await expect(editForm.getByText('No program grants.')).toBeVisible();
  });
});
