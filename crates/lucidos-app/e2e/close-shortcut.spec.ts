import { test, expect, type Page } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, waitForVisibleInput, isMobileViewport } from './helpers';
import { clearAllThreads } from './db-helpers';

/** Cross-platform Cmd/Ctrl+Shift+W. Matches the `e.metaKey || e.ctrlKey`
 *  guard in useKeyboardShortcuts.ts. */
async function pressCloseShortcut(page: Page): Promise<void> {
  await page.keyboard.press('ControlOrMeta+Shift+W');
}

async function getFocusedThreadId(page: Page): Promise<string> {
  const id = await (await waitForVisibleInput(page)).getAttribute('data-thread-id');
  expect(id, 'focused thread id missing on visible prompt input').toBeTruthy();
  return id!;
}

async function waitForThreadInArchiveSection(page: Page, threadId: string): Promise<void> {
  await page.waitForFunction(
    (id) => {
      const titles = Array.from(document.querySelectorAll('[data-flip-id="__section_archive"]'));
      return titles.some(t => t.closest('.drawer-section')?.querySelector(`[data-thread-nav="${id}"]`));
    },
    threadId,
    { timeout: 10_000 },
  );
}

test.describe('Cmd/Ctrl+Shift+W — close focused thread', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('archives the focused active thread', async ({ page }) => {
    // Mobile drawer is hidden by default; section assertion needs the drawer.
    test.skip(isMobileViewport(page), 'desktop-only: relies on always-visible drawer');

    await navigateToApp(page);
    const msg = uniqueMessage('close-shortcut');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    const threadId = await getFocusedThreadId(page);

    await pressCloseShortcut(page);

    await waitForThreadInArchiveSection(page, threadId);
  });
});
