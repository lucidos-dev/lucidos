import { test, expect, Page, Locator } from './fixtures';
import {
  navigateToApp,
  assertHealthy,
  openTriggersPanel,
  addTriggerCard,
  pickDropdownOption,
} from './helpers';

/** Trigger group lifecycle through the real panel UI (TriggersView +
 *  TriggerGroupHeader + the TriggerDetails group picker).
 *
 *  - Creation: the "New Group" card → inline name field → Enter lands an empty
 *    section header with a "(0)" badge.
 *  - Deletion: an EMPTY group's delete button is enabled; clicking it +
 *    confirming removes the section.
 *  - The full lifecycle additionally pins the member-count guard: a group with
 *    a member trigger shows "(1)" and its delete button is DISABLED, and only
 *    becomes deletable once the trigger is removed. That guard is the server's
 *    409-on-non-empty rule surfaced in the UI, so it's the most important thing
 *    to keep regression-tested. */

// Every name this spec creates starts with this prefix so afterEach can find
// and remove them — the e2e DB resets only between Playwright projects, not
// between tests, so leftovers would pollute sibling tests' /trigger-groups poll.
const PREFIX = 'e2e-grp';

/** A trigger-group section identified by its header name (dual-layout safe via
 *  the visible-only DOM; ContentPane mounts the panel once). The filter keys on
 *  the header's `.trigger-group-name` span — trigger rows inside the section use
 *  `.list-row-name`, so a member trigger never makes the filter ambiguous. */
function groupSection(page: Page, name: string): Locator {
  return page.locator('.trigger-group-section').filter({
    has: page.locator('.trigger-group-name', { hasText: name }),
  });
}

/** Accept whatever confirm dialog is currently open (group/trigger deletes are
 *  both guarded by `showConfirm`, whose default OK button is `.confirm-btn-ok`). */
async function confirmDialog(page: Page): Promise<void> {
  await page.locator('.confirm-dialog').waitFor({ state: 'visible', timeout: 10_000 });
  await page.locator('.confirm-dialog .confirm-btn-ok:visible').first().click();
}

/** Dismiss any open toasts. On mobile the toast container overlays the panel and
 *  intercepts clicks on the group/trigger action buttons near the top; the info
 *  "created" toast auto-dismisses after 5s, but we clear it eagerly so the next
 *  click never races that window. */
async function clearToasts(page: Page): Promise<void> {
  for (let i = 0; i < 5; i++) {
    const close = page.locator('.toast .toast-close').first();
    if (!(await close.isVisible().catch(() => false))) return;
    await close.click({ timeout: 2_000 }).catch(() => {});
  }
}

/** Create a group via the "New Group" card and wait for its empty section.
 *  Clears the resulting "created" toast so callers can click panel buttons. */
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
    // create_trigger reads the timezone preference; set it so the create path
    // never falls back to its UTC default mid-test (matches trigger-side-effects).
    await page.request.put('/api/v1/preferences?key=timezone', { data: { value: 'UTC' } });
  });

  test.afterEach(async ({ page }) => {
    // Best-effort cleanup: triggers first (so their groups become empty), then
    // groups — a non-empty group refuses deletion (409).
    try {
      const tRes = await page.request.get('/api/v1/triggers');
      const tBody = await tRes.json();
      for (const t of (tBody.triggers ?? []) as Array<{ id: string; name: string }>) {
        if (t.name?.startsWith(PREFIX)) await page.request.delete(`/api/v1/triggers?id=${t.id}`);
      }
      const gRes = await page.request.get('/api/v1/trigger-groups');
      const gBody = await gRes.json();
      for (const g of (gBody.groups ?? []) as Array<{ id: string; name: string }>) {
        if (g.name?.startsWith(PREFIX)) await page.request.delete(`/api/v1/trigger-groups?id=${g.id}`);
      }
    } catch {
      /* best-effort cleanup */
    }
  });

  test('creates an empty group and deletes it', async ({ page }) => {
    const groupName = `${PREFIX}-${Date.now()}`;

    await navigateToApp(page);
    await openTriggersPanel(page);

    // Create — empty group lands with a (0) badge.
    await createGroup(page, groupName);
    const section = groupSection(page, groupName);
    await expect(section.locator('.trigger-group-count')).toHaveText('(0)');

    // Delete: an empty group's delete icon is enabled; confirm removes the section.
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

    // 1. Create group A.
    await createGroup(page, groupA);

    // 2. Rename A → B. Clicking the rename icon reveals the edit field over the
    //    name (so the section can no longer be found by name A). EVERY heading
    //    carries that field mounted and hidden, which is what lets the tap focus
    //    it and open the mobile keyboard, so target the one heading that is
    //    actually renaming rather than the class alone.
    await groupSection(page, groupA).locator('.trigger-group-rename').click();
    const renameInput = page.locator('.trigger-group-renaming .trigger-group-name-input');
    await expect(renameInput).toBeVisible({ timeout: 5_000 });
    await renameInput.fill(groupB);
    await renameInput.press('Enter');
    await expect(groupSection(page, groupB)).toBeVisible({ timeout: 10_000 });
    await expect(groupSection(page, groupA)).toHaveCount(0);

    // 3. Create a trigger assigned to group B via the form's Group picker.
    await addTriggerCard(page).click();
    const form = page.locator('.inline-form:visible').first();
    await expect(form).toBeVisible({ timeout: 10_000 });
    await form.locator('input[placeholder="e.g. Morning Brief"]').fill(triggerName);
    await form.locator('input[placeholder="0 0 8 * * *"]').fill('0 0 8 * * *');
    await form.locator('.prompt-textarea').fill('Send me a hello every morning');
    await pickDropdownOption(page, '.trigger-group-select', groupB);
    await form.locator('.btn-save').click();

    // The trigger lands under group B, the badge flips to (1), and (the guard)
    // delete is now DISABLED (the panel mirrors the server's non-empty refusal).
    const sectionB = groupSection(page, groupB);
    await expect(sectionB.locator('.trigger-row .list-row-name', { hasText: triggerName }))
      .toBeVisible({ timeout: 10_000 });
    await expect(sectionB.locator('.trigger-group-count')).toHaveText('(1)', { timeout: 10_000 });
    await expect(sectionB.locator('.trigger-group-delete')).toBeDisabled();

    // 4. Empty the group by deleting its member trigger.
    await clearToasts(page);
    await sectionB.locator('.trigger-row', { hasText: triggerName })
      .locator('.list-row-actions .action-btn-danger')
      .click();
    await confirmDialog(page);
    await expect(sectionB.locator('.trigger-row', { hasText: triggerName }))
      .toHaveCount(0, { timeout: 10_000 });

    // Badge back to (0); delete re-enabled.
    await expect(sectionB.locator('.trigger-group-count')).toHaveText('(0)', { timeout: 10_000 });
    const deleteB = sectionB.locator('.trigger-group-delete');
    await expect(deleteB).toBeEnabled();

    // 5. Delete the now-empty group.
    await deleteB.click();
    await confirmDialog(page);
    await expect(sectionB).toHaveCount(0, { timeout: 10_000 });
  });
});
