import { test, expect, type Page, type Locator } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer, showGrouping, showOngoingGroup, waitForVisibleInput, ensureOnThreadPane, clickVisibleElement, clickThreadRow, threadRowFor, isMobileViewport, composerControl, closeComposerFold } from './helpers';
import { clearAllThreads, psql } from './db-helpers';
import { KEEPS_THE_SERVICE_WORKER } from './harnessStamp';

test.use(KEEPS_THE_SERVICE_WORKER);

async function startThreadThenCompose(
  page: Page,
  prefix: string,
  draftText: string,
): Promise<{ promptInput: Locator; activeId: string; draftId: string }> {
  await navigateToApp(page);

  const msg = uniqueMessage(prefix);
  await sendMessage(page, `Say exactly: "${msg}"`);
  await waitForResponse(page);

  const promptInput = page.locator('[data-role="prompt-input"]:visible').first();
  const activeId = await promptInput.getAttribute('data-thread-id');
  if (!activeId) throw new Error('Active thread input missing data-thread-id');

  await newThread(page);
  const composeInput = await waitForVisibleInput(page);
  await composeInput.fill(draftText);

  await expect(promptInput).toHaveAttribute('data-thread-id', /.+/, { timeout: 5_000 });
  const draftId = await promptInput.getAttribute('data-thread-id');
  if (!draftId) throw new Error('Compose input missing data-thread-id');
  expect(draftId).not.toBe(activeId);

  return { promptInput, activeId, draftId };
}

async function clickThreadNav(page: Page, ariaLabel: 'Previous thread' | 'Next thread'): Promise<void> {
  const enabledSelector = `button[aria-label="${ariaLabel}"]:not([disabled])`;
  await expect(page.locator(`${enabledSelector}:visible`).first()).toBeEnabled({ timeout: 5_000 });
  if (!await clickVisibleElement(page, enabledSelector)) {
    throw new Error(`${ariaLabel} button not visible`);
  }
}

async function clearDraft(page: Page): Promise<void> {
  await (await composerControl(page, 'button.prompt-clear')).click();
}

async function expectClearDraftOffered(page: Page): Promise<void> {
  await expect(await composerControl(page, 'button.prompt-clear')).toBeVisible();
  await closeComposerFold(page);
}

test.describe('Per-thread drafts', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('thread draft persists when switching to compose and back', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('draft-persist');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    const threadId = await input.getAttribute('data-thread-id');
    if (!threadId) throw new Error('Active thread input missing data-thread-id');
    await input.fill('thread draft text');

    await newThread(page);
    const composeInput = await waitForVisibleInput(page);
    await expect(composeInput).toHaveValue('');

    await openThreadDrawer(page);
    await clickThreadRow(page, threadId);
    await ensureOnThreadPane(page);

    const threadInput = await waitForVisibleInput(page);
    await expect(threadInput).toHaveAttribute('data-thread-id', threadId);
    try {
      await expect(threadInput).toHaveValue('thread draft text');
    } catch (assertErr) {
      let persisted: string;
      try {
        persisted = psql(`SELECT compose_text FROM thread_summaries WHERE thread_id = '${threadId}'`);
      } catch (psqlErr) {
        persisted = `<persisted-draft query failed: ${(psqlErr as Error).message}>`;
      }
      const domValue = await threadInput.inputValue().catch(() => '<unreadable>');
      const face = persisted === 'thread draft text'
        ? 'CLOBBER (persisted server-side but absent from the textarea after 30s — a local clear-path wiped/never-restored the draft; product bug)'
        : 'NOT-STORED (no persisted draft — the compose PUT never landed: fill()->updateCompose race or failed PUT)';
      throw new Error(
        `drafts:65 draft-restore FAILED — face: ${face}. ` +
        `textarea value=${JSON.stringify(domValue)}, persisted compose_text=${JSON.stringify(persisted)}, ` +
        `thread=${threadId}. Original: ${(assertErr as Error).message}`,
      );
    }
  });

  test('Compose always opens a fresh blank draft, preserving previous compose drafts', async ({ page }) => {
    await navigateToApp(page);

    const first = await waitForVisibleInput(page);
    await first.fill('first compose draft');

    await newThread(page);
    const second = await waitForVisibleInput(page);
    await expect(second).toHaveValue('');

    await second.fill('second compose draft');

    await newThread(page);
    const third = await waitForVisibleInput(page);
    await expect(third).toHaveValue('');

    await openThreadDrawer(page);
    const currentSection = page.locator('.list-section-title:visible', { hasText: 'Current' });
    await expect(currentSection).toBeVisible({ timeout: 5_000 });

    const firstRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'first compose draft' });
    const secondRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'second compose draft' });
    await expect(firstRow).toBeVisible({ timeout: 5_000 });
    await expect(secondRow).toBeVisible({ timeout: 5_000 });
  });

  test('clicking a compose draft row in the drawer restores that draft', async ({ page }) => {
    await navigateToApp(page);

    const input = await waitForVisibleInput(page);
    await input.fill('return to me');
    await newThread(page);

    await openThreadDrawer(page);
    const savedRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'return to me' });
    await expect(savedRow).toBeVisible({ timeout: 5_000 });
    const clicked = await clickVisibleElement(page, '.compose-draft-row', 'return to me');
    if (!clicked) throw new Error('Saved compose draft row not clickable');
    await ensureOnThreadPane(page);

    const restored = await waitForVisibleInput(page);
    await expect(restored).toHaveValue('return to me', { timeout: 5_000 });
  });

  test('existing thread → New → type → clear (auto-discard) → Back returns to the existing thread', async ({ page }) => {
    const { promptInput, activeId } = await startThreadThenCompose(page, 'discard-back', 'soon to be discarded');

    await clearDraft(page);
    const cleared = await waitForVisibleInput(page);
    await expect(cleared).toHaveValue('', { timeout: 5_000 });

    await clickThreadNav(page, 'Previous thread');
    await expect(promptInput).toHaveAttribute('data-thread-id', activeId, { timeout: 5_000 });
  });

  test('existing thread → New → type → Back lands on the existing thread, Forward returns to the draft', async ({ page }) => {
    const { promptInput, activeId, draftId } = await startThreadThenCompose(page, 'nav-draft', 'draft text');

    await clickThreadNav(page, 'Previous thread');
    await expect(promptInput).toHaveAttribute('data-thread-id', activeId, { timeout: 5_000 });

    await clickThreadNav(page, 'Next thread');
    await expect(promptInput).toHaveAttribute('data-thread-id', draftId, { timeout: 5_000 });
  });

  test('compose draft is cleared and removed from the drawer after sending', async ({ page }) => {
    await navigateToApp(page);

    const input = await waitForVisibleInput(page);
    await input.fill('will be sent');

    const msg = uniqueMessage('draft-clear');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    await newThread(page);
    const composeInput = await waitForVisibleInput(page);
    await expect(composeInput).toHaveValue('');

    await openThreadDrawer(page);
    const stale = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'will be sent' });
    await expect(stale).toHaveCount(0);
  });

  test('draft indicator shows on thread rows with thread-attached drafts', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('draft-indicator');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    const threadId = await input.getAttribute('data-thread-id');
    if (!threadId) throw new Error('Active thread input missing data-thread-id');
    await input.fill('unsent draft');

    await newThread(page);

    await openThreadDrawer(page);
    const draftIndicator = page.locator(`${threadRowFor(threadId)}:visible .draft-indicator`).first();
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
    const composeInput = await waitForVisibleInput(page);
    await composeInput.fill('new compose with text');

    await openThreadDrawer(page);
    const draftRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'new compose with text' });
    await expect(draftRow).toBeVisible({ timeout: 5_000 });
  });

  test('focused thread draft visibility in Drafts section depends on viewport', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('drafts-focused');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    await input.fill('focused draft');

    await openThreadDrawer(page);

    if (isMobileViewport(page)) {
      await showOngoingGroup(page, 'Drafts');
      const draftsTile = page.locator('.ongoing-tile:visible', { hasText: 'Drafts' });
      await expect(draftsTile).toBeVisible({ timeout: 5_000 });
    } else {
      const visibleInput = await waitForVisibleInput(page);
      await expect(visibleInput).toHaveValue('focused draft', { timeout: 5_000 });
    }
  });

  test('compose draft row title comes from the draft text, not a placeholder', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('compose-draft-row');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const sentInput = await waitForVisibleInput(page);
    const threadId = await sentInput.getAttribute('data-thread-id');
    if (!threadId) throw new Error('Active thread input missing data-thread-id');

    await newThread(page);

    const input = await waitForVisibleInput(page);
    await input.fill('compose only draft');

    await openThreadDrawer(page);
    await clickThreadRow(page, threadId);
    await ensureOnThreadPane(page);

    await openThreadDrawer(page);
    const currentSection = page.locator('.list-section-title:visible', { hasText: 'Current' });
    await expect(currentSection).toBeVisible({ timeout: 5_000 });

    const titledRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'compose only draft' });
    await expect(titledRow).toBeVisible({ timeout: 5_000 });
  });

  test('compose draft falls back to "New thread" when text is empty', async ({ page }) => {
    await navigateToApp(page);

    const input = await waitForVisibleInput(page);
    await input.fill('temporary');
    await input.fill('');

    await newThread(page);
    await openThreadDrawer(page);
    const draftRow = page.locator('.compose-draft-row:visible');
    await expect(draftRow).toHaveCount(0, { timeout: 2_000 });
  });

  test('Clearing the text discards the compose draft and removes it from the panel', async ({ page }) => {
    await navigateToApp(page);

    const input = await waitForVisibleInput(page);
    await input.fill('about to be discarded');
    await newThread(page);

    await openThreadDrawer(page);
    const established = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'about to be discarded' });
    await expect(established).toBeVisible({ timeout: 5_000 });
    const clicked = await clickVisibleElement(page, '.compose-draft-row', 'about to be discarded');
    if (!clicked) throw new Error('established compose draft row not clickable');
    await ensureOnThreadPane(page);

    const restored = await waitForVisibleInput(page);
    await expect(restored).toHaveValue('about to be discarded', { timeout: 5_000 });

    await clearDraft(page);

    const cleared = await waitForVisibleInput(page);
    await expect(cleared).toHaveValue('', { timeout: 5_000 });

    await openThreadDrawer(page);
    const stale = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'about to be discarded' });
    await expect(stale).toHaveCount(0);
  });

  test('Clearing a follow-up draft clears the compose without deleting the active thread', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('discard-thread-mode');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    await input.fill('thread follow-up to discard');

    await expectClearDraftOffered(page);

    await clearDraft(page);

    const cleared = await waitForVisibleInput(page);
    await expect(cleared).toHaveValue('', { timeout: 5_000 });

    await expect(page.locator('.toast-error')).toHaveCount(0, { timeout: 1_000 });

    await expect(cleared).toHaveAttribute('placeholder', 'Post a follow up…');

    await page.reload();
    await navigateToApp(page);
    const reloaded = await waitForVisibleInput(page);
    await expect(reloaded).toHaveValue('', { timeout: 10_000 });
    await expect(reloaded).toHaveAttribute('placeholder', 'Post a follow up…');
  });

  test('Compose Send does not leave a stale draft row that resurrects on reload', async ({ page }) => {
    await navigateToApp(page);

    const input = await waitForVisibleInput(page);
    const draftText = uniqueMessage('csc');
    await input.fill(draftText);
    await newThread(page);
    const fresh = await waitForVisibleInput(page);
    await expect(fresh).toHaveValue('');

    await openThreadDrawer(page);
    const established = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: draftText });
    await expect(established).toBeVisible({ timeout: 5_000 });
    const clicked = await clickVisibleElement(page, '.compose-draft-row', draftText);
    if (!clicked) throw new Error('established compose draft row not clickable');
    await ensureOnThreadPane(page);
    const restored = await waitForVisibleInput(page);
    await expect(restored).toHaveValue(draftText, { timeout: 5_000 });
    await sendMessage(page, draftText);
    await waitForResponse(page);

    await page.reload();
    await navigateToApp(page);
    await openThreadDrawer(page);
    const stale = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: draftText });
    await expect(stale).toHaveCount(0);
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

  test('thread draft fills the textarea even when it is focused before composeText loads', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('draft-focused-reload');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    await input.fill('persists with focus');

    await page.reload();
    await navigateToApp(page);

    const reloadedInput = await waitForVisibleInput(page);
    await reloadedInput.focus();
    await expect(reloadedInput).toHaveValue('persists with focus', { timeout: 10_000 });
    await expectClearDraftOffered(page);
  });

  test('compose draft survives page reload — same thread id, not a new one', async ({ page }) => {
    await navigateToApp(page);

    const composeInput = await waitForVisibleInput(page);
    const composePut = page.waitForResponse((res) => /\/api\/v1\/threads\/[^/]+\/compose$/.test(res.url()) && res.request().method() === 'PUT' && res.ok());
    await composeInput.fill('compose persists across reload');

    await expect(composeInput).toHaveAttribute('data-thread-id', /.+/, { timeout: 5_000 });
    const beforeReloadId = await composeInput.getAttribute('data-thread-id');
    if (!beforeReloadId) throw new Error('compose input missing data-thread-id');
    await composePut;

    await page.reload();
    await navigateToApp(page);

    const reloadedInput = await waitForVisibleInput(page);
    await expect(reloadedInput).toHaveAttribute('data-thread-id', beforeReloadId, { timeout: 10_000 });
    await expect(reloadedInput).toHaveValue('compose persists across reload');
  });

  test('focused compose draft renders the compose view, not a thread header', async ({ page }) => {
    await navigateToApp(page);

    const input = await waitForVisibleInput(page);
    await input.fill('Selecting a thread');
    await newThread(page);

    await openThreadDrawer(page);
    const draftRow = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'Selecting a thread' });
    await expect(draftRow).toBeVisible({ timeout: 5_000 });
    const clicked = await clickVisibleElement(page, '.compose-draft-row', 'Selecting a thread');
    if (!clicked) throw new Error('Compose draft row not clickable');
    await ensureOnThreadPane(page);

    const pane = page.locator('.thread-pane.compose-empty:visible').first();
    await expect(pane).toBeVisible({ timeout: 5_000 });

    await expect(page.locator('.thread-view-header')).toHaveCount(0);
    await expect(pane.getByText('Untitled Thread')).toHaveCount(0);
    await expect(page.getByText('No messages in this thread')).toHaveCount(0);

    const restored = await waitForVisibleInput(page);
    await expect(restored).toHaveValue('Selecting a thread', { timeout: 5_000 });

    await restored.fill('');
    await expect(draftRow).toHaveCount(0, { timeout: 5_000 });
    await expect(page.locator('.thread-title:visible')).toHaveCount(0);
    await expect(page.getByText('Empty draft')).toHaveCount(0);
  });

  test('an empty Ongoing still draws every tile, and the selected one says it is empty', async ({ page }) => {
    await navigateToApp(page);
    await openThreadDrawer(page);
    await showGrouping(page, 'ongoing');
    await expect(page.locator('.drawer-ongoing-tiles .ongoing-tile:visible')).toHaveCount(4);
    await expect(page.locator('.ongoing-grouping .empty-state:visible')).toHaveText('Nothing is blocked');
  });
});
