import { test, expect, Page, Locator } from './fixtures';
import {
  addTriggerCard, apiRequest, assertHealthy, navigateToApp, openTriggersPanel,
  pickDropdownOption, waitForModalExitFade,
} from './helpers';


const PREFIX = 'e2e-grp';

function groupSection(page: Page, name: string): Locator {
  return page.locator('.trigger-group-section').filter({
    has: page.locator('.trigger-group-name', { hasText: name }),
  });
}

async function confirmDialog(page: Page): Promise<void> {
  await page.locator('.confirm-dialog').waitFor({ state: 'visible', timeout: 10_000 });
  await page.locator('.confirm-dialog [data-role="confirm-ok"]:visible').first().click();
  await waitForModalExitFade(page);
}

async function clearToasts(page: Page): Promise<void> {
  for (let i = 0; i < 5; i++) {
    const close = page.locator('.toast .toast-close').first();
    if (!(await close.isVisible().catch(() => false))) break;
    await close.click({ timeout: 2_000 }).catch(() => {});
  }
  await page.locator('.toast').first().waitFor({ state: 'detached', timeout: 7_000 }).catch(() => {});
}

async function createGroup(page: Page, name: string): Promise<void> {
  await page.locator('.list-row-add-card:visible', { hasText: 'New Group' }).first().click();
  const input = page.locator('.trigger-group-create-row .trigger-group-name-input');
  await expect(input).toBeVisible({ timeout: 5_000 });
  await input.fill(name);
  await input.press('Enter');
  await expect(groupSection(page, name)).toBeVisible({ timeout: 10_000 });
  await clearToasts(page);
}

test.describe('Trigger groups — create / delete', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await apiRequest(page).put('/api/v1/preferences?key=timezone', { data: { value: 'UTC' } });
  });

  test.afterEach(async ({ page }) => {
    try {
      const tRes = await page.request.get('/api/v1/triggers');
      const tBody = await tRes.json();
      for (const t of (tBody.triggers ?? []) as Array<{ id: string; name: string }>) {
        if (t.name?.startsWith(PREFIX)) await apiRequest(page).delete(`/api/v1/triggers?id=${t.id}`);
      }
      const gRes = await page.request.get('/api/v1/trigger-groups');
      const gBody = await gRes.json();
      for (const g of (gBody.groups ?? []) as Array<{ id: string; name: string }>) {
        if (g.name?.startsWith(PREFIX)) await apiRequest(page).delete(`/api/v1/trigger-groups?id=${g.id}`);
      }
    } catch {
    }
  });

  test('creates an empty group and deletes it', async ({ page }) => {
    const groupName = `${PREFIX}-${Date.now()}`;

    await navigateToApp(page);
    await openTriggersPanel(page);

    await createGroup(page, groupName);
    const section = groupSection(page, groupName);
    await expect(section.locator('.section-count-open')).toHaveText('0');

    const deleteBtn = section.locator('.trigger-group-delete');
    await expect(deleteBtn).toBeEnabled();
    await deleteBtn.click();
    await confirmDialog(page);
    await expect(section).toHaveCount(0, { timeout: 10_000 });
  });

  test('create → rename → assign trigger → delete', async ({ page }) => {
    const groupA = `${PREFIX}-${Date.now()}`;
    const groupB = `${PREFIX}-renamed-${Date.now()}`;
    const triggerName = `${PREFIX}-trigger-${Date.now()}`;

    await navigateToApp(page);
    await openTriggersPanel(page);

    await createGroup(page, groupA);

    await groupSection(page, groupA).locator('.trigger-group-rename').click();
    const renameInput = page.locator('.trigger-group-renaming .trigger-group-name-input');
    await expect(renameInput).toBeVisible({ timeout: 5_000 });
    await renameInput.fill(groupB);
    await renameInput.press('Enter');
    await expect(groupSection(page, groupB)).toBeVisible({ timeout: 10_000 });
    await expect(groupSection(page, groupA)).toHaveCount(0);

    await addTriggerCard(page).click();
    const form = page.locator('.inline-form:visible').first();
    await expect(form).toBeVisible({ timeout: 10_000 });
    await form.locator('input[placeholder="e.g. Morning Brief"]').fill(triggerName);
    await form.locator('input[placeholder="0 0 8 * * *"]').fill('0 0 8 * * *');
    await form.locator('.prompt-textarea').fill('Send me a hello every morning');
    await pickDropdownOption(page, '.trigger-group-select', groupB);
    await form.locator('.btn-save').click();

    const sectionB = groupSection(page, groupB);
    await expect(sectionB.locator('.trigger-row .list-row-name', { hasText: triggerName }))
      .toBeVisible({ timeout: 10_000 });
    await expect(sectionB.locator('.section-count-open')).toHaveText('1', { timeout: 10_000 });
    const deleteWhileFull = sectionB.locator('.trigger-group-delete');
    await expect(deleteWhileFull).toBeEnabled();
    await expect(deleteWhileFull).toHaveAttribute('data-tooltip', 'Move triggers out first');

    await clearToasts(page);
    await deleteWhileFull.click();
    await confirmDialog(page);
    await expect(page.locator('.toast', { hasText: 'Move or delete the 1 trigger' }))
      .toBeVisible({ timeout: 10_000 });
    await expect(sectionB).toHaveCount(1);
    await expect(sectionB.locator('.section-count-open')).toHaveText('1');

    await clearToasts(page);
    await sectionB.locator('.trigger-row', { hasText: triggerName })
      .locator('.list-row-actions .action-btn-danger')
      .click();
    await confirmDialog(page);
    await expect(sectionB.locator('.trigger-row', { hasText: triggerName }))
      .toHaveCount(0, { timeout: 10_000 });

    await expect(sectionB.locator('.section-count-open')).toHaveText('0', { timeout: 10_000 });
    const deleteB = sectionB.locator('.trigger-group-delete');
    await expect(deleteB).toBeEnabled();

    await deleteB.click();
    await confirmDialog(page);
    await expect(sectionB).toHaveCount(0, { timeout: 10_000 });
  });
});
