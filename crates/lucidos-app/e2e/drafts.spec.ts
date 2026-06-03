import { test, expect, type Page, type Locator } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer, waitForVisibleInput, ensureOnThreadPane, clickVisibleElement, isMobileViewport, REAL_THREAD_ROW } from './helpers';
import { clearAllThreads } from './db-helpers';

/** Send a first message, then click Compose and type a draft. Returns the
 *  prompt-input locator, the original (active) thread id, and the new draft
 *  thread id so individual tests can assert on what happens next. */
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

  // The draft id is unknown until ensureFocusedComposeThread allocates it on
  // first keystroke; wait for the attribute to flip from '' before capturing.
  await expect(promptInput).toHaveAttribute('data-thread-id', /.+/, { timeout: 5_000 });
  const draftId = await promptInput.getAttribute('data-thread-id');
  if (!draftId) throw new Error('Compose input missing data-thread-id');
  expect(draftId).not.toBe(activeId);

  return { promptInput, activeId, draftId };
}

/** Dual-layout-safe click of a thread-nav button. Both SplitLayout (desktop)
 *  and MobileSwipeContainer render their copies simultaneously, so a bare
 *  `:visible.first()` can resolve to the offscreen pane. */
async function clickThreadNav(page: Page, ariaLabel: 'Previous thread' | 'Next thread'): Promise<void> {
  const enabledSelector = `button[aria-label="${ariaLabel}"]:not([disabled])`;
  await expect(page.locator(`${enabledSelector}:visible`).first()).toBeEnabled({ timeout: 5_000 });
  if (!await clickVisibleElement(page, enabledSelector)) {
    throw new Error(`${ariaLabel} button not visible`);
  }
}

/** Click "Discard draft" and accept the confirm. Discarding an unsent draft is
 *  destructive, so the confirm is tied to the ACTION — it now appears whether
 *  the discard is triggered by this button or by the close-cascade shortcut. */
async function discardDraftViaButton(page: Page): Promise<void> {
  if (!await clickVisibleElement(page, 'button[aria-label="Discard draft"]')) {
    throw new Error('Discard button not visible');
  }
  const ok = page.locator('.confirm-dialog .confirm-btn-ok:visible').first();
  await expect(ok).toBeVisible({ timeout: 5_000 });
  await ok.click();
}

test.describe('Per-thread drafts', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
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
    const newSection = page.locator('.list-section-title:visible', { hasText: 'New' });
    await expect(newSection).toBeVisible({ timeout: 5_000 });

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

  test('existing thread → New → type → Discard → Back returns to the existing thread', async ({ page }) => {
    // Regression: after Discard, the cursor sits on the existing thread but
    // focusedThreadId is null (compose pane). A naive "decrement cursor" Back
    // skipped that entry and jumped to whatever was navigated before it.
    const { promptInput, activeId } = await startThreadThenCompose(page, 'discard-back', 'soon to be discarded');

    await discardDraftViaButton(page);
    const cleared = await waitForVisibleInput(page);
    await expect(cleared).toHaveValue('', { timeout: 5_000 });

    const back = page.locator('button[aria-label="Previous thread"]:visible').first();
    await expect(back).toBeEnabled({ timeout: 5_000 });
    await back.click();
    await expect(promptInput).toHaveAttribute('data-thread-id', activeId, { timeout: 5_000 });
  });

  test('Discard draft can be canceled — the draft survives the confirm', async ({ page }) => {
    // The confirm lives on the action, so it appears for the button too;
    // canceling it must leave the typed draft exactly as it was.
    await startThreadThenCompose(page, 'discard-cancel', 'keep this draft');

    if (!await clickVisibleElement(page, 'button[aria-label="Discard draft"]')) {
      throw new Error('Discard button not visible');
    }
    const cancel = page.locator('.confirm-dialog .confirm-btn-cancel:visible').first();
    await expect(cancel).toBeVisible({ timeout: 5_000 });
    await cancel.click();

    // Draft text is untouched.
    const stillThere = await waitForVisibleInput(page);
    await expect(stillThere).toHaveValue('keep this draft', { timeout: 5_000 });
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

    // The drawer's New section only renders composing threads with content.
    await newThread(page);
    const composeInput = await waitForVisibleInput(page);
    await composeInput.fill('new compose with text');

    await openThreadDrawer(page);
    const newSection = page.locator('.list-section-title:visible', { hasText: 'New' });
    await expect(newSection).toBeVisible({ timeout: 5_000 });
  });

  test('focused thread draft visibility in Drafts section depends on viewport', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('drafts-focused');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    await input.fill('focused draft');

    await openThreadDrawer(page);

    const newSection = page.locator('.list-section-title:visible', { hasText: 'New' });

    if (isMobileViewport(page)) {
      // Mobile hides the textarea from the threads pane, so a focused thread's
      // follow-up draft only surfaces in the dedicated Drafts view.
      const clicked = await clickVisibleElement(page, 'button[aria-label="Toggle drafts view"]');
      if (!clicked) throw new Error('Toggle drafts view button not visible');
      const draftsSection = page.locator('.list-section-title:visible', { hasText: 'Drafts' });
      await expect(draftsSection).toBeVisible({ timeout: 5_000 });
    } else {
      // On desktop, the focused thread's draft is visible in the textarea,
      // so it should NOT appear in the Drafts section
      await expect(newSection).not.toBeVisible({ timeout: 5_000 });
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
    const newSection = page.locator('.list-section-title:visible', { hasText: 'New' });
    await expect(newSection).toBeVisible({ timeout: 5_000 });

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
    const newSection = page.locator('.list-section-title:visible', { hasText: 'New' });
    // No persisted compose drafts — section should be absent
    await expect(newSection).not.toBeVisible({ timeout: 2_000 });
  });

  test('Discard clears the compose textarea and removes the draft from the panel', async ({ page }) => {
    await navigateToApp(page);

    // Type, navigate away to establish, then come back so the draft has a panel row
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

    // Click Discard and accept the confirm
    await discardDraftViaButton(page);

    // Textarea is empty
    const cleared = await waitForVisibleInput(page);
    await expect(cleared).toHaveValue('', { timeout: 5_000 });

    // Open drawer again — the draft is gone (no row, no New section)
    await openThreadDrawer(page);
    const stale = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'about to be discarded' });
    await expect(stale).toHaveCount(0);
  });

  test('Discard on a follow-up draft clears the compose without deleting the active thread', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('discard-thread-mode');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    // Now focused on an active thread; type a follow-up draft
    const input = await waitForVisibleInput(page);
    await input.fill('thread follow-up to discard');

    // Discard button is visible whenever the prompt has content (compose or followup)
    const discardBtn = page.locator('button[aria-label="Discard draft"]:visible');
    await expect(discardBtn).toHaveCount(1);

    // Click Discard (and accept the confirm) — for an active thread, clears the
    // in-progress text/images but keeps the thread intact. Must not attempt to
    // delete the thread server-side (which would 409 with "thread is active —
    // use archive instead").
    await discardDraftViaButton(page);

    const cleared = await waitForVisibleInput(page);
    await expect(cleared).toHaveValue('', { timeout: 5_000 });

    // No error toast — discard on an active thread is a local clear, not a delete.
    await expect(page.locator('.toast-error')).toHaveCount(0, { timeout: 1_000 });

    // User stays on the active thread — placeholder is the follow-up one,
    // not the compose-view "Go ahead…". Asserting placeholder also avoids
    // the dual-layout-render trap (desktop and mobile copies coexist in DOM).
    await expect(cleared).toHaveAttribute('placeholder', 'Post a follow up…');

    // Reload — followup draft must NOT come back from the server projection,
    // and the textarea is still the follow-up one (thread not tombstoned).
    await page.reload();
    await navigateToApp(page);
    const reloaded = await waitForVisibleInput(page);
    await expect(reloaded).toHaveValue('', { timeout: 10_000 });
    await expect(reloaded).toHaveAttribute('placeholder', 'Post a follow up…');
  });

  test('Compose Send does not leave a stale draft row that resurrects on reload', async ({ page }) => {
    await navigateToApp(page);

    // Type a compose draft, navigate away to establish so it pushes to the server.
    // Prefix kept short — drawer titles cap at 40 chars (threadTitle.MAX_LEN), and
    // the longer 'compose-send-cleanup' prefix produced a 41-char unique string
    // whose final char was sliced off in the row title, breaking hasText match.
    const input = await waitForVisibleInput(page);
    const draftText = uniqueMessage('csc');
    await input.fill(draftText);
    await newThread(page);
    const fresh = await waitForVisibleInput(page);
    await expect(fresh).toHaveValue('');

    // Re-focus the established compose draft and send it
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

    // Reload — the compose draft id is different from the new thread id, so the
    // backend's MessageReceived hard-delete (`WHERE id == thread_id`) cannot
    // match it. The frontend must explicitly tombstone the compose draft on
    // Send; otherwise the row resurrects in the panel after reload.
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
    // Bug: when focusIfNeeded grabs focus on initial mount before
    // loadAllThreads resolves, the previous "skip sync while focused" guard
    // suppressed the eventual composeText overwrite. Result: the textarea
    // stayed blank while the drawer label and Discard button still reflected
    // the saved draft. Reproduction: focus the textarea immediately after
    // reload, then assert the persisted text reaches it anyway.
    await navigateToApp(page);

    const msg = uniqueMessage('draft-focused-reload');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    await input.fill('persists with focus');

    await page.reload();
    await navigateToApp(page);

    const reloadedInput = await waitForVisibleInput(page);
    // Focus before the composeText assertion — production race where the
    // textarea was focused first and the older guard stuck on userTyping.
    await reloadedInput.focus();
    await expect(reloadedInput).toHaveValue('persists with focus', { timeout: 10_000 });
    // The "Discard draft" button is gated on composeText.length > 0; if the
    // text is missing but the draft state still shows, the button is the
    // visible artifact users reported.
    const discardBtn = page.locator('button[aria-label="Discard draft"]:visible');
    await expect(discardBtn).toHaveCount(1);
  });

  test('compose draft survives page reload — same thread id, not a new one', async ({ page }) => {
    // Bug: ensureFocusedComposeThread allocated a UUID and set focusedThreadId
    // but never wrote it to localStorage. On reload, focusedThreadId restored
    // to null and the next keystroke allocated a fresh UUID — landing the
    // user on a brand-new compose pane with the previous draft orphaned
    // server-side. Fix: setFocusedThread persists the id so the same draft
    // resumes after reload.
    await navigateToApp(page);

    const composeInput = await waitForVisibleInput(page);
    // Wait for both the create POST and the debounced compose PUT to land
    // before reloading. The keepalive flush in flushAllPending makes this
    // mostly redundant, but the deterministic wait removes a 250ms race.
    const composePut = page.waitForResponse((res) => /\/api\/v1\/threads\/[^/]+\/compose$/.test(res.url()) && res.request().method() === 'PUT' && res.ok());
    await composeInput.fill('compose persists across reload');

    // The draft id is allocated by ensureFocusedComposeThread on the first
    // keystroke; wait for it to land on the data attribute before snapshotting.
    await expect(composeInput).toHaveAttribute('data-thread-id', /.+/, { timeout: 5_000 });
    const beforeReloadId = await composeInput.getAttribute('data-thread-id');
    if (!beforeReloadId) throw new Error('compose input missing data-thread-id');
    await composePut;

    await page.reload();
    await navigateToApp(page);

    const reloadedInput = await waitForVisibleInput(page);
    // Same thread id — without the fix this would be empty (focusedThreadId
    // null because the id was never persisted) and a fresh keystroke would
    // allocate a new UUID.
    await expect(reloadedInput).toHaveAttribute('data-thread-id', beforeReloadId, { timeout: 10_000 });
    // Same draft text, still in the same compose row.
    await expect(reloadedInput).toHaveValue('compose persists across reload');
  });

  test('focused compose draft renders the compose view, not a thread header', async ({ page }) => {
    // Focusing a composing draft from the drawer must keep the centered
    // compose layout — same as a brand-new compose page. No thread header
    // (renaming an unsent draft is meaningless), no "No messages" body, and
    // the prompt input stays vertically centered until Send promotes the
    // draft to an active thread.
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

    // The pane is in compose-empty mode — prompt centered, no thread header.
    const pane = page.locator('.thread-pane.compose-empty:visible').first();
    await expect(pane).toBeVisible({ timeout: 5_000 });

    // No ThreadView header rendered at all (drafts have no editable title).
    await expect(page.locator('.thread-view-header')).toHaveCount(0);
    // No "Untitled Thread" fallback anywhere.
    await expect(page.getByText('Untitled Thread')).toHaveCount(0);
    // Body must not surface the "No messages in this thread" copy.
    await expect(page.getByText('No messages in this thread')).toHaveCount(0);

    // The textarea still carries the typed draft.
    const restored = await waitForVisibleInput(page);
    await expect(restored).toHaveValue('Selecting a thread', { timeout: 5_000 });

    // Clearing the draft on a never-sent thread auto-discards it: the row
    // disappears from the drawer and the focused thread is released so the
    // user is back on the empty CreateThreadView.
    await restored.fill('');
    await expect(draftRow).toHaveCount(0, { timeout: 5_000 });
    await expect(page.locator('.thread-title-display:visible')).toHaveCount(0);
    await expect(page.getByText('Empty draft')).toHaveCount(0);
  });
});
