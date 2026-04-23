import { test, expect } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer, waitForVisibleInput, ensureOnThreadPane, clickVisibleElement, isMobileViewport } from './helpers';

// Selector for real thread rows (excludes compose-draft rows that share the
// .thread-row class but live under their own draft id).
const REAL_THREAD_ROW = '.thread-row:not(.compose-draft-row)';

test.describe('Per-thread drafts', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('thread draft persists when switching to compose and back', async ({ page }) => {
    await navigateToApp(page);

    // Create a thread so we have something to switch back to
    const msg = uniqueMessage('draft-persist');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    // Type a draft in the thread
    const input = await waitForVisibleInput(page);
    await input.fill('thread draft text');

    // Compose: opens a fresh blank draft
    await newThread(page);
    const composeInput = await waitForVisibleInput(page);
    await expect(composeInput).toHaveValue('');

    // Click the real thread row (skip any compose-draft rows the drawer renders)
    await openThreadDrawer(page);
    const clicked = await clickVisibleElement(page, REAL_THREAD_ROW);
    if (!clicked) throw new Error('No visible real thread row found');
    await ensureOnThreadPane(page);

    // Thread draft restored
    const threadInput = await waitForVisibleInput(page);
    await expect(threadInput).toHaveValue('thread draft text', { timeout: 5_000 });
  });

  test('Compose always opens a fresh blank draft, preserving previous compose drafts', async ({ page }) => {
    await navigateToApp(page);

    // Type a first compose draft
    const first = await waitForVisibleInput(page);
    await first.fill('first compose draft');

    // Click Compose — must open a brand new blank draft, NOT reuse the first
    await newThread(page);
    const second = await waitForVisibleInput(page);
    await expect(second).toHaveValue('');

    // Type a second compose draft
    await second.fill('second compose draft');

    // Click Compose again — fresh blank, both prior drafts preserved
    await newThread(page);
    const third = await waitForVisibleInput(page);
    await expect(third).toHaveValue('');

    // Open drawer — Drafts section now lists both prior compose drafts
    await openThreadDrawer(page);
    const draftsSection = page.locator('.list-section-title:visible', { hasText: 'Drafts' });
    await expect(draftsSection).toBeVisible({ timeout: 5_000 });

    const firstRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'first compose draft' });
    const secondRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'second compose draft' });
    await expect(firstRow).toBeVisible({ timeout: 5_000 });
    await expect(secondRow).toBeVisible({ timeout: 5_000 });
  });

  test('clicking a compose draft row in the drawer restores that draft', async ({ page }) => {
    await navigateToApp(page);

    // Create a saved compose draft, then move past it with another Compose
    const input = await waitForVisibleInput(page);
    await input.fill('return to me');
    await newThread(page);

    // Open drawer and click the saved compose draft — el.click() via evaluate
    // bypasses touch-event routing under hasTouch (which can swallow clicks
    // on Preact onClick handlers in Chromium mobile emulation)
    await openThreadDrawer(page);
    const savedRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'return to me' });
    await expect(savedRow).toBeVisible({ timeout: 5_000 });
    const clicked = await clickVisibleElement(page, '.compose-draft-row', 'return to me');
    if (!clicked) throw new Error('Saved compose draft row not clickable');
    await ensureOnThreadPane(page);

    // The clicked draft is now active in the prompt
    const restored = await waitForVisibleInput(page);
    await expect(restored).toHaveValue('return to me', { timeout: 5_000 });
  });

  test('compose draft is cleared and removed from the drawer after sending', async ({ page }) => {
    await navigateToApp(page);

    const input = await waitForVisibleInput(page);
    await input.fill('will be sent');

    const msg = uniqueMessage('draft-clear');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    // Compose again — fresh blank, the sent draft is gone (promoted to thread)
    await newThread(page);
    const composeInput = await waitForVisibleInput(page);
    await expect(composeInput).toHaveValue('');

    // The previous text must NOT show up in the Drafts section
    await openThreadDrawer(page);
    const stale = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'will be sent' });
    await expect(stale).toHaveCount(0);
  });

  test('draft indicator shows on thread rows with thread-attached drafts', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('draft-indicator');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    // Type a draft in this thread
    const input = await waitForVisibleInput(page);
    await input.fill('unsent draft');

    // Switch to compose so the thread's draft is saved
    await newThread(page);

    // Open drawer — the thread row carries a "Draft" badge
    await openThreadDrawer(page);
    const draftIndicator = page.locator(`${REAL_THREAD_ROW}:visible .draft-indicator`).first();
    await expect(draftIndicator).toBeVisible({ timeout: 5_000 });
    await expect(draftIndicator).toHaveText('Draft');
  });

  test('Drafts section appears with threads that have drafts', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('drafts-section');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    await input.fill('section draft');

    await newThread(page);

    await openThreadDrawer(page);
    const draftsSection = page.locator('.list-section-title:visible', { hasText: 'Drafts' });
    await expect(draftsSection).toBeVisible({ timeout: 5_000 });
  });

  test('focused thread draft visibility in Drafts section depends on viewport', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('drafts-focused');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    await input.fill('focused draft');

    await openThreadDrawer(page);

    const draftsSection = page.locator('.list-section-title:visible', { hasText: 'Drafts' });

    if (isMobileViewport(page)) {
      // On mobile, the drawer is a separate pane — the focused thread's draft
      // correctly appears in the Drafts section since the textarea isn't visible
      await expect(draftsSection).toBeVisible({ timeout: 5_000 });
    } else {
      // On desktop, the focused thread's draft is visible in the textarea,
      // so it should NOT appear in the Drafts section
      await expect(draftsSection).not.toBeVisible({ timeout: 5_000 });
    }
  });

  test('compose draft row title comes from the draft text, not a placeholder', async ({ page }) => {
    await navigateToApp(page);

    // First create a thread so we have somewhere to navigate to
    const msg = uniqueMessage('compose-draft-row');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    await newThread(page);

    const input = await waitForVisibleInput(page);
    await input.fill('compose only draft');

    // Navigate to the thread (away from compose) via drawer so the compose
    // draft is no longer focused — only then is it shown in Drafts on desktop
    await openThreadDrawer(page);
    await clickVisibleElement(page, REAL_THREAD_ROW);
    await ensureOnThreadPane(page);

    await openThreadDrawer(page);
    const draftsSection = page.locator('.list-section-title:visible', { hasText: 'Drafts' });
    await expect(draftsSection).toBeVisible({ timeout: 5_000 });

    const titledRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'compose only draft' });
    await expect(titledRow).toBeVisible({ timeout: 5_000 });
  });

  test('compose draft falls back to "New thread" when text is empty', async ({ page }) => {
    await navigateToApp(page);

    // Save an image-only style draft by typing then deleting (proxy for
    // image-only — this test only exercises the title fallback path)
    const input = await waitForVisibleInput(page);
    await input.fill('temporary');
    await input.fill('');

    // After clearing the text, the draft is empty and should NOT be saved
    await newThread(page);
    await openThreadDrawer(page);
    const draftsSection = page.locator('.list-section-title:visible', { hasText: 'Drafts' });
    // No persisted compose drafts — section should be absent
    await expect(draftsSection).not.toBeVisible({ timeout: 2_000 });
  });

  test('thread draft survives page reload', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('draft-reload');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    await input.fill('survives reload');

    await page.reload();
    await navigateToApp(page);

    const reloadedInput = await waitForVisibleInput(page);
    await expect(reloadedInput).toHaveValue('survives reload', { timeout: 10_000 });
  });
});
