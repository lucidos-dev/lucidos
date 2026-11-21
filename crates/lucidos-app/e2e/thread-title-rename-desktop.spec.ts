import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  waitForResponse,
  uniqueMessage,
  clickVisibleElement,
  waitForThreadTitle,
  getVisibleTitleText,
  openThreadDrawer,
  renameThreadViaMenu,
} from './helpers';

/** Desktop rename and the drawer-aware title row. The `-desktop` suffix keeps
 *  these off the mobile projects, where `.thread-view-header` is not drawn.
 *  The title never edits in place. It opens the thread menu, and Rename…
 *  there opens the prompt dialog prefilled with the title. */

async function newThread(page: import('@playwright/test').Page, tag: string): Promise<void> {
  await navigateToApp(page);
  await sendMessage(page, `Say exactly: "${uniqueMessage(tag)}"`);
  await waitForResponse(page);
  await waitForThreadTitle(page);
}

test.describe('Thread title and rename: desktop', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('renames through the thread menu, from a dialog prefilled with the title', async ({ page }) => {
    await newThread(page, 'rename');
    const before = await getVisibleTitleText(page);

    await clickVisibleElement(page, '.thread-view-header .thread-title-menu');
    await page.locator('.thread-overflow-menu [role="menuitem"]', { hasText: 'Rename' }).first().click();
    const field = page.locator('.confirm-dialog .prompt-input');
    await expect(field).toHaveValue(before);

    await field.fill(`Renamed ${Date.now()}`);
    const next = await field.inputValue();
    await page.locator('[data-role="prompt-ok"]').click();
    await expect.poll(() => getVisibleTitleText(page)).toBe(next);
  });

  test('cancelling the dialog keeps the title', async ({ page }) => {
    await newThread(page, 'rename-cancel');
    const before = await getVisibleTitleText(page);

    await clickVisibleElement(page, '.thread-view-header .thread-title-menu');
    await page.locator('.thread-overflow-menu [role="menuitem"]', { hasText: 'Rename' }).first().click();
    await page.locator('.confirm-dialog .prompt-input').fill('Not this one');
    await page.locator('[data-role="prompt-cancel"]').click();

    await expect(page.locator('.confirm-dialog')).toHaveCount(0);
    expect(await getVisibleTitleText(page)).toBe(before);
  });

  test('clicking the title opens the thread menu, not an editor', async ({ page }) => {
    await newThread(page, 'title-click');
    await expect(page.locator('.thread-view-header [aria-label="More thread actions"]')).toHaveCount(0);
    const title = page.locator('.thread-view-header .thread-title');
    await expect(title).toHaveCSS('cursor', 'pointer');
    await title.click();
    await expect(page.locator('.thread-overflow-menu [role="menuitem"]', { hasText: 'Rename' })).toBeVisible();
    await expect(page.locator('.thread-view-header input, .thread-view-header textarea')).toHaveCount(0);
    await expect(page.locator('.confirm-dialog')).toHaveCount(0);
  });

  test('right-clicking the title opens the thread menu', async ({ page }) => {
    await newThread(page, 'title-right-click');
    await page.locator('.thread-view-header .thread-title').click({ button: 'right' });
    await expect(page.locator('.thread-overflow-menu [role="menuitem"]', { hasText: 'Rename' })).toBeVisible();
  });

  test('the title row stays while the drawer is open, and rename works from there', async ({ page }) => {
    await newThread(page, 'drawer-open');
    await openThreadDrawer(page);

    // The open thread's drawer row can be scrolled, folded or filtered away,
    // so the title row, whose title opens the menu, stays on screen.
    await expect(page.locator('.thread-view-header .thread-title')).toBeVisible();

    const next = `From the title row ${Date.now()}`;
    await renameThreadViaMenu(page, next);
    await expect.poll(() => getVisibleTitleText(page)).toBe(next);
  });
});
