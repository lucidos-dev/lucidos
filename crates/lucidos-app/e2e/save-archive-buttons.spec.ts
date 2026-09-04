import { test, expect, Page } from './fixtures';
import {
  apiRequest, assertHealthy, ensureOnThreadPane, navigateToApp, openThreadDrawer, sendMessage,
  uniqueMessage, waitForResponse, waitForVisibleInput,
} from './helpers';

test.describe('Section-aware Save/Archive buttons', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  async function getFocusedThreadId(page: Page): Promise<string> {
    const id = await (await waitForVisibleInput(page)).getAttribute('data-thread-id');
    expect(id, 'focused thread id missing on visible prompt input').toBeTruthy();
    return id!;
  }

  async function waitForThreadInSection(
    page: Page,
    threadId: string,
    sectionKey: 'saved' | 'archive' | 'current',
  ): Promise<void> {
    await page.waitForFunction(
      ({ id, key }) => {
        const titles = Array.from(document.querySelectorAll(`[data-flip-id="__section_${key}"]`));
        return titles.some(t => t.closest('.drawer-section')?.querySelector(`[data-thread-nav="${id}"]`));
      },
      { id: threadId, key: sectionKey },
      { timeout: 10_000 },
    );
  }

  async function isThreadInSection(
    page: Page,
    threadId: string,
    sectionKey: 'saved' | 'archive' | 'current',
  ): Promise<boolean> {
    return page.evaluate(({ id, key }) => {
      const titles = Array.from(document.querySelectorAll(`[data-flip-id="__section_${key}"]`));
      return titles.some(t => !!t.closest('.drawer-section')?.querySelector(`[data-thread-nav="${id}"]`));
    }, { id: threadId, key: sectionKey });
  }

  async function focusThreadFromDrawer(page: Page, threadId: string): Promise<void> {
    await openThreadDrawer(page);
    await page.locator(`[data-thread-nav="${threadId}"]:visible`).first().click();
    await ensureOnThreadPane(page);
    await page.waitForFunction((id) => {
      const inputs = document.querySelectorAll('[data-role="prompt-input"]');
      return Array.from(inputs).some(el =>
        el.getBoundingClientRect().width > 0 && el.getAttribute('data-thread-id') === id,
      );
    }, threadId, { timeout: 10_000 });
  }

  test('pinning an archived thread moves it to the Pinned section', async ({ page }) => {
    await navigateToApp(page);
    const msg = uniqueMessage('arch-save');
    await sendMessage(page, `Echo "${msg}"`);
    await waitForResponse(page);
    const threadId = await getFocusedThreadId(page);

    const archResp = await apiRequest(page).post('/api/v1/threads/archive', {
      data: { thread_id: threadId },
    });
    expect(archResp.ok()).toBeTruthy();
    // Desktop chromium hides the drawer by default — open it so
    // waitForThreadInSection can find the section's DOM. Mobile/webkit
    // projects keep the drawer mounted, so openThreadDrawer is a no-op
    // there.
    await openThreadDrawer(page);
    await waitForThreadInSection(page, threadId, 'archive');

    await focusThreadFromDrawer(page, threadId);

    // Scope pin selectors to the focused thread's header actions — the drawer
    // rows each carry their own pin button now, and the drawer is open here (and
    // always mounted on mobile), so an unscoped selector counts/clicks drawer-row
    // pins of OTHER threads. Archive lives only in the prompt row, so its
    // aria-label is unambiguous and stays unscoped.
    await expect(page.locator('.thread-view-header-actions button[aria-label="Pin thread"]:visible').first()).toBeVisible();
    expect(await page.locator('button[aria-label="Archive thread"]:visible').count()).toBe(0);

    await page.locator('.thread-view-header-actions button[aria-label="Pin thread"]:visible').first().click();
    await waitForThreadInSection(page, threadId, 'saved');
    expect(await isThreadInSection(page, threadId, 'saved')).toBe(true);
  });

  test('archiving a pinned thread asks for confirmation and demotes to Archive', async ({ page }) => {
    await navigateToApp(page);
    const msg = uniqueMessage('saved-arch');
    await sendMessage(page, `Echo "${msg}"`);
    await waitForResponse(page);
    const threadId = await getFocusedThreadId(page);

    const saveResp = await apiRequest(page).post('/api/v1/threads/save', {
      data: { thread_id: threadId },
    });
    expect(saveResp.ok()).toBeTruthy();
    // Desktop chromium hides the drawer by default — open it so
    // waitForThreadInSection can find the section's DOM. Mobile/webkit
    // projects keep the drawer mounted, so openThreadDrawer is a no-op
    // there.
    await openThreadDrawer(page);
    await waitForThreadInSection(page, threadId, 'saved');

    await focusThreadFromDrawer(page, threadId);

    await expect(page.locator('button[aria-label="Archive thread"]:visible').first()).toBeVisible();
    // Pinned threads always carry the "✓ Pinned" unpin toggle alongside Archive
    // so the user can drop back to regular flow at any time.
    await expect(
      page.locator('.thread-view-header-actions button[aria-label="Remove thread from Pinned section"]:visible').first(),
    ).toBeVisible();
    expect(await page.locator('.thread-view-header-actions button[aria-label="Pin thread"]:visible').count()).toBe(0);

    await page.locator('button[aria-label="Archive thread"]:visible').first().click();
    await expect(
      page.getByText('Are you sure you want to move this thread to the archive?'),
    ).toBeVisible({ timeout: 5_000 });

    // Two Archive buttons exist while the dialog is open — scope to the dialog.
    await page.locator('.confirm-btn.confirm-btn-ok:visible').first().click();

    await waitForThreadInSection(page, threadId, 'archive');
    expect(await isThreadInSection(page, threadId, 'archive')).toBe(true);
    expect(await isThreadInSection(page, threadId, 'saved')).toBe(false);
  });
});
