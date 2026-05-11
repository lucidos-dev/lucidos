import { test, expect, type Page } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, waitForVisibleInput, isMobileViewport, waitForThreadInSection } from './helpers';
import { clearAllThreads } from './db-helpers';

async function getFocusedThreadId(page: Page): Promise<string> {
  const id = await (await waitForVisibleInput(page)).getAttribute('data-thread-id');
  expect(id, 'focused thread id missing on visible prompt input').toBeTruthy();
  return id!;
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

    // ControlOrMeta = Cmd on macOS, Ctrl elsewhere — matches the
    // `e.metaKey || e.ctrlKey` guard in useKeyboardShortcuts.ts.
    await page.keyboard.press('ControlOrMeta+Shift+W');

    await waitForThreadInSection(page, threadId, 'archive');
  });
});
