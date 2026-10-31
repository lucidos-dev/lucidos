import { test, expect, Page } from './fixtures';
import {
  apiRequest, assertHealthy, ensureOnThreadPane, navigateToApp, openThreadDrawer, sendMessage,
  uniqueMessage, waitForResponse, waitForVisibleInput, toggleFocusedPin, expectFocusedPinned } from './helpers';

test.describe('Section-aware Save/Archive buttons', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  async function getFocusedThreadId(page: Page): Promise<string> {
    const id = await (await waitForVisibleInput(page)).getAttribute('data-thread-id');
    expect(id, 'focused thread id missing on visible prompt input').toBeTruthy();
    return id!;
  }

  type SectionKey = 'saved' | 'archive' | 'current';

  /** Wait until the section's rows near its header are mounted, then return
   *  whether `threadId` is among them, or null while they are not.
   *
   *  The drawer mounts only the rows near its viewport. Once earlier specs
   *  fill Current, Archive sits below that window with no rows in the DOM.
   *  So the poll scrolls the header into view. The thread under test is the
   *  newest in its section, so it is the first row below the header. */
  async function waitForSectionAnswer(page: Page, threadId: string, sectionKey: SectionKey): Promise<boolean> {
    const handle = await page.waitForFunction(
      ({ id, key }) => {
        const titles = Array.from(document.querySelectorAll<HTMLElement>(`[data-flip-id="__section_${key}"]`))
          .filter(t => t.getBoundingClientRect().height > 0);
        // No section header at all: the section holds nothing.
        if (titles.length === 0) return 'absent';
        for (const title of titles) {
          const section = title.closest('.drawer-section');
          if (section?.querySelector(`[data-thread-nav="${id}"]`)) return 'present';
          if (section?.querySelector('[data-thread-nav]')) return 'other';
          title.scrollIntoView({ block: 'start' });
        }
        return null;
      },
      { id: threadId, key: sectionKey },
      { timeout: 10_000 },
    );
    return (await handle.jsonValue()) === 'present';
  }

  async function waitForThreadInSection(page: Page, threadId: string, sectionKey: SectionKey): Promise<void> {
    await expect.poll(() => waitForSectionAnswer(page, threadId, sectionKey), { timeout: 10_000 }).toBe(true);
  }

  async function expectThreadNotInSection(page: Page, threadId: string, sectionKey: SectionKey): Promise<void> {
    expect(await waitForSectionAnswer(page, threadId, sectionKey)).toBe(false);
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
    // Desktop hides the drawer by default. Open it so
    // waitForThreadInSection can find the section's DOM.
    await openThreadDrawer(page);
    await waitForThreadInSection(page, threadId, 'archive');

    await focusThreadFromDrawer(page, threadId);

    // The pin helpers act on the focused thread only: every drawer row carries
    // its own pin. Archive lives only in the prompt row, so its aria-label is
    // unambiguous and stays unscoped.
    await expectFocusedPinned(page, false);
    expect(await page.locator('button[aria-label="Archive thread"]:visible').count()).toBe(0);

    await toggleFocusedPin(page, 'pinned');
    await waitForThreadInSection(page, threadId, 'saved');
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
    // Desktop hides the drawer by default. Open it so
    // waitForThreadInSection can find the section's DOM.
    await openThreadDrawer(page);
    await waitForThreadInSection(page, threadId, 'saved');

    await focusThreadFromDrawer(page, threadId);

    await expect(page.locator('button[aria-label="Archive thread"]:visible').first()).toBeVisible();
    // A pinned thread always offers Unpin alongside Archive, so the user can
    // drop back to the regular flow at any time.
    await expectFocusedPinned(page, true);

    await page.locator('button[aria-label="Archive thread"]:visible').first().click();
    await expect(
      page.getByText('Archiving unpins this thread. Move it to the archive?'),
    ).toBeVisible({ timeout: 5_000 });

    // Two Archive buttons exist while the dialog is open — scope to the dialog.
    await page.locator('[data-role="confirm-ok"]:visible').first().click();

    await waitForThreadInSection(page, threadId, 'archive');
    await expectThreadNotInSection(page, threadId, 'saved');
  });
});
