import { test, expect, type Page, type Locator } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer, openDrawerView, waitForVisibleInput, ensureOnThreadPane, clickVisibleElement, clickThreadRow, threadRowFor, isMobileViewport } from './helpers';
import { clearAllThreads, psql } from './db-helpers';

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

/** Clear the prompt via the clear-X in the composer's prompt row, which is the
 *  discard affordance. Emptying a never-sent compose draft auto-discards it,
 *  since updateCompose routes an empty patch through discardCompose. On an
 *  active thread it is a local clear of the follow-up text, with no confirm. */
async function clearDraft(page: Page): Promise<void> {
  if (!await clickVisibleElement(page, 'button.prompt-clear')) {
    throw new Error('Clear button not visible');
  }
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
    const threadId = await input.getAttribute('data-thread-id');
    if (!threadId) throw new Error('Active thread input missing data-thread-id');
    await input.fill('thread draft text');

    // Compose: opens a fresh blank draft
    await newThread(page);
    const composeInput = await waitForVisibleInput(page);
    await expect(composeInput).toHaveValue('');

    // Back to THIS thread, BY ID, never "the first visible real thread row".
    // The e2e workspace is shared and `clearAllThreads()` truncates only the
    // thread_summaries projection. So a coding-agent session still running from
    // an earlier spec can re-insert its row with `last_activity = NOW()` and
    // outrank ours. See `clickThreadRow`.
    await openThreadDrawer(page);
    await clickThreadRow(page, threadId);
    await ensureOnThreadPane(page);

    // Thread draft restored. The timeout is the suite's default `expect`
    // timeout (30s, playwright.config.ts), NOT an explicit 5s. On the contended
    // nightly the mobile-webkit textarea hydration can lag behind a WebContent
    // starvation freeze (docs/e2e-test-decisions.md, "mobile-webkit navigation
    // wedge"). That is a slow-but-correct restore and should pass. A genuine
    // clobber or not-stored bug leaves the draft empty FOREVER, so the longer
    // wait sharpens the signal rather than masking one.
    const threadInput = await waitForVisibleInput(page);
    // IDENTITY BEFORE VALUE. An assertion about "the draft" says nothing if the
    // app parked us on a different thread. A wrong-thread landing must never be
    // reported as a lost draft. Polls, since the restore re-renders, so a
    // slow-but-correct switch still passes.
    await expect(threadInput).toHaveAttribute('data-thread-id', threadId);
    try {
      await expect(threadInput).toHaveValue('thread draft text');
    } catch (assertErr) {
      // Classify the failure FACE so a future nightly flake self-diagnoses. The
      // textarea binds to the local composeDrafts signal, so an empty textarea
      // after the full 30s is NOT a transient paint stall. Query the PERSISTED
      // draft of the thread ASSERTED ON above, never the restored id, to split
      // the two faces:
      //   • persisted === the draft: CLOBBER, stored server-side but wiped from
      //     the local signal, a product clear-path bug.
      //   • persisted === '': NOT-STORED, the PUT never landed. Either a
      //     fill() to updateCompose race, or a failed PUT.
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

    // Open drawer — the Current section now lists both prior compose drafts at
    // the top.
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

    // Create a saved compose draft, then move past it with another Compose
    const input = await waitForVisibleInput(page);
    await input.fill('return to me');
    await newThread(page);

    // Open the drawer and click the saved compose draft. el.click() via
    // evaluate bypasses touch-event routing under hasTouch, which can swallow
    // clicks on Preact onClick handlers in Chromium mobile emulation.
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

  test('existing thread → New → type → clear (auto-discard) → Back returns to the existing thread', async ({ page }) => {
    // Regression: after the draft is discarded, the cursor sits on the existing
    // thread but focusedThreadId is null (compose pane). A naive "decrement
    // cursor" Back skipped that entry and jumped to whatever was navigated
    // before it. Clearing the compose text auto-discards the never-sent draft.
    const { promptInput, activeId } = await startThreadThenCompose(page, 'discard-back', 'soon to be discarded');

    await clearDraft(page);
    const cleared = await waitForVisibleInput(page);
    await expect(cleared).toHaveValue('', { timeout: 5_000 });

    // Dual-layout-safe: a raw `.first().click()` resolves to the offscreen
    // layout's nav button, which the visible thread-pane-body intercepts on
    // mobile. clickThreadNav uses a synthetic el.click that fires the handler
    // regardless.
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
    const threadId = await input.getAttribute('data-thread-id');
    if (!threadId) throw new Error('Active thread input missing data-thread-id');
    await input.fill('unsent draft');

    // Switch to compose so the thread's draft is saved
    await newThread(page);

    // Open the drawer: THIS thread's row carries a "Draft" badge. Scoped to our
    // own row, since a foreign thread's row can outrank ours in the drawer (see
    // `threadRowFor`).
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

    // The drawer only renders composing threads with content; they ride at the
    // top of the Current section.
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
      // Mobile hides the textarea from the threads pane, so a focused thread's
      // follow-up draft only surfaces in the dedicated Drafts view.
      await openDrawerView(page, 'Drafts');
      const draftsSection = page.locator('.list-section-title:visible', { hasText: 'Drafts' });
      await expect(draftsSection).toBeVisible({ timeout: 5_000 });
    } else {
      // On desktop, the focused thread's follow-up draft is shown inline in the
      // visible textarea — no drafts view needed to see it.
      const visibleInput = await waitForVisibleInput(page);
      await expect(visibleInput).toHaveValue('focused draft', { timeout: 5_000 });
    }
  });

  test('compose draft row title comes from the draft text, not a placeholder', async ({ page }) => {
    await navigateToApp(page);

    // First create a thread so we have somewhere to navigate to
    const msg = uniqueMessage('compose-draft-row');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const sentInput = await waitForVisibleInput(page);
    const threadId = await sentInput.getAttribute('data-thread-id');
    if (!threadId) throw new Error('Active thread input missing data-thread-id');

    await newThread(page);

    const input = await waitForVisibleInput(page);
    await input.fill('compose only draft');

    // Navigate to the thread through the drawer so the compose draft is no
    // longer focused. Only then does desktop show it in Drafts. By id, not by
    // position, per `clickThreadRow`.
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

    // Save an image-only style draft by typing then deleting. This test only
    // exercises the title fallback path.
    const input = await waitForVisibleInput(page);
    await input.fill('temporary');
    await input.fill('');

    // After clearing the text, the draft is empty and should NOT be saved
    await newThread(page);
    await openThreadDrawer(page);
    // No persisted compose drafts and no other threads — no draft row renders.
    const draftRow = page.locator('.compose-draft-row:visible');
    await expect(draftRow).toHaveCount(0, { timeout: 2_000 });
  });

  test('Clearing the text discards the compose draft and removes it from the panel', async ({ page }) => {
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

    // Clear the text: emptying a never-sent compose draft auto-discards it.
    await clearDraft(page);

    // Textarea is empty
    const cleared = await waitForVisibleInput(page);
    await expect(cleared).toHaveValue('', { timeout: 5_000 });

    // Open drawer again — the draft is gone (no compose-draft row)
    await openThreadDrawer(page);
    const stale = page.locator('.compose-draft-row:visible .thread-row-title', { hasText: 'about to be discarded' });
    await expect(stale).toHaveCount(0);
  });

  test('Clearing a follow-up draft clears the compose without deleting the active thread', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('discard-thread-mode');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    // Now focused on an active thread; type a follow-up draft
    const input = await waitForVisibleInput(page);
    await input.fill('thread follow-up to discard');

    // The clear-X is visible whenever the prompt has text (compose or followup)
    const clearBtn = page.locator('button.prompt-clear:visible');
    await expect(clearBtn).toHaveCount(1);

    // Clear the follow-up text. For an active thread this is a local clear that
    // keeps the thread intact, never a server-side delete. That would 409 on an
    // active thread and point at archive instead.
    await clearDraft(page);

    const cleared = await waitForVisibleInput(page);
    await expect(cleared).toHaveValue('', { timeout: 5_000 });

    // No error toast: discard on an active thread is a local clear, not a delete.
    await expect(page.locator('.toast-error')).toHaveCount(0, { timeout: 1_000 });

    // The user stays on the active thread, so the placeholder is the follow-up
    // one, not the compose view's. Asserting the placeholder also avoids the
    // dual-layout-render trap, where desktop and mobile copies coexist.
    await expect(cleared).toHaveAttribute('placeholder', 'Post a follow up…');

    // Reload: the follow-up draft must NOT come back from the server
    // projection, and the textarea is still the follow-up one.
    await page.reload();
    await navigateToApp(page);
    const reloaded = await waitForVisibleInput(page);
    await expect(reloaded).toHaveValue('', { timeout: 10_000 });
    await expect(reloaded).toHaveAttribute('placeholder', 'Post a follow up…');
  });

  test('Compose Send does not leave a stale draft row that resurrects on reload', async ({ page }) => {
    await navigateToApp(page);

    // Type a compose draft, then navigate away so it pushes to the server. The
    // prefix is kept short because drawer titles cap at 40 chars
    // (threadTitle.MAX_LEN), and a sliced title breaks the hasText match.
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

    // Reload. The compose draft id differs from the new thread id, so the
    // backend's MessageReceived hard-delete (`WHERE id == thread_id`) cannot
    // match it. The frontend must tombstone the compose draft on Send, or the
    // row resurrects in the panel after reload.
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
    // focusIfNeeded can grab focus on initial mount before loadAllThreads
    // resolves. The persisted text must still reach the textarea, so focus it
    // immediately after reload and then assert.
    await navigateToApp(page);

    const msg = uniqueMessage('draft-focused-reload');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const input = await waitForVisibleInput(page);
    await input.fill('persists with focus');

    await page.reload();
    await navigateToApp(page);

    const reloadedInput = await waitForVisibleInput(page);
    // Focus before the composeText assertion, reproducing the race where the
    // textarea is focused first.
    await reloadedInput.focus();
    await expect(reloadedInput).toHaveValue('persists with focus', { timeout: 10_000 });
    // The clear-X is gated on composeText.length > 0, so it is the visible
    // artifact when the text is missing while the draft state still shows.
    const clearBtn = page.locator('button.prompt-clear:visible');
    await expect(clearBtn).toHaveCount(1);
  });

  test('compose draft survives page reload — same thread id, not a new one', async ({ page }) => {
    // setFocusedThread must persist the id ensureFocusedComposeThread
    // allocates. Without that, reload restores focusedThreadId to null, the
    // next keystroke allocates a fresh UUID, and the previous draft is orphaned
    // server-side.
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
    // Same thread id. Without the persisted id this is empty and a fresh
    // keystroke allocates a new UUID.
    await expect(reloadedInput).toHaveAttribute('data-thread-id', beforeReloadId, { timeout: 10_000 });
    // Same draft text, still in the same compose row.
    await expect(reloadedInput).toHaveValue('compose persists across reload');
  });

  test('focused compose draft renders the compose view, not a thread header', async ({ page }) => {
    // Focusing a composing draft from the drawer must keep the centered compose
    // layout, exactly as a brand-new compose page does. No thread header, since
    // renaming an unsent draft is meaningless. No "No messages" body, and the
    // prompt input stays centered until Send promotes the draft.
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
    // Scoped to the PANE, which is what the claim is about. The drawer this
    // test opened lists every thread in the workspace. A titleless one with no
    // message to derive from legitimately reads "Untitled Thread" there (see
    // `threadTitle.ts`). Page-wide, a row a sibling spec left behind failed
    // this on the first attempt of all three projects.
    await expect(pane.getByText('Untitled Thread')).toHaveCount(0);
    // Body must not surface the "No messages in this thread" copy. PAGE-wide,
    // unlike the line above, and the asymmetry is load-bearing. Only
    // `ThreadView` renders that copy, and `ThreadPane` renders `ThreadView`
    // and `compose-empty` on opposite branches. Scoped to the pane the check
    // could never match, so it would assert nothing. Page-wide it still
    // catches a `ThreadView` in the other pane of the mobile layout.
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

  test('Filter control is always present and opens an empty Drafts view with no drafts', async ({ page }) => {
    // beforeEach cleared all threads, so there are zero drafts. The Filter
    // control is always present, and picking the Drafts view opens it to its
    // own empty state.
    await navigateToApp(page);
    await openThreadDrawer(page);

    const headerSel = isMobileViewport(page) ? '.mobile-threads-header' : '.threads-header';
    const filterBtn = page.locator(`${headerSel} button[aria-label="Filter threads"]`);
    await expect(filterBtn).toBeVisible({ timeout: 5_000 });

    await openDrawerView(page, 'Drafts');
    await expect(page.locator('.empty-state:visible', { hasText: 'No drafts' }))
      .toBeVisible({ timeout: 5_000 });
  });
});
