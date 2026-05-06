import { test, expect } from '@playwright/test';
import { assertHealthy, navigateToApp, sendMessage, waitForResponse, uniqueMessage, clickVisibleElement, blurActiveElement } from './helpers';

/** Wait for a visible .thread-title-display (read-only <textarea>) with a
 *  non-empty value (dual-layout safe). */
async function waitForThreadTitle(page: import('@playwright/test').Page, timeout = 30_000) {
  await page.waitForFunction(() => {
    const els = document.querySelectorAll('.thread-title-display');
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && ((el as HTMLTextAreaElement).value ?? '').trim().length > 0;
    });
  }, undefined, { timeout });
}

/** Wait for the thread title editor to enter edit mode (wrapper gains .is-editing). */
async function waitForTitleInput(page: import('@playwright/test').Page, timeout = 5_000) {
  await page.waitForFunction(() => {
    const wrappers = document.querySelectorAll('.thread-title-edit.is-editing');
    return Array.from(wrappers).some(w => {
      const rect = w.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout });
  return page.locator('.thread-title-edit.is-editing .thread-title-edit-input').first();
}

/** Get the visible thread title text from the display textarea. */
async function getVisibleTitleText(page: import('@playwright/test').Page): Promise<string> {
  return page.evaluate(() => {
    const els = document.querySelectorAll('.thread-title-display');
    for (const el of els) {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        return ((el as HTMLTextAreaElement).value ?? '').trim();
      }
    }
    return '';
  });
}

/** Height (px) of the visible mobile thread-title-display textarea, ignoring
 *  the desktop copy. Returns 0 if neither layout's display is rendered. */
async function getMobileTitleHeight(page: import('@playwright/test').Page): Promise<number> {
  return page.evaluate(() => {
    const els = document.querySelectorAll('.mobile-thread-title-row .thread-title-display');
    for (const el of els) {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0) return rect.height;
    }
    return 0;
  });
}

test.describe('Thread title editing — desktop', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('click title to edit, type new name, press Enter to save', async ({ page }) => {
    await navigateToApp(page);

    // Send a message to create a thread
    const msg = uniqueMessage('title-edit');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    // Wait for the title to appear (auto-generated)
    await waitForThreadTitle(page);

    // Click the title to enter edit mode
    await clickVisibleElement(page, '.thread-title-display');
    const input = await waitForTitleInput(page);

    // Type a new title
    const newTitle = `Renamed ${Date.now()}`;
    await input.fill(newTitle);
    await input.press('Enter');

    // Wait for the title display to show the new name
    await page.waitForFunction((expected) => {
      const els = document.querySelectorAll('.thread-title-display');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && ((el as HTMLTextAreaElement).value ?? '').trim() === expected;
      });
    }, newTitle, { timeout: 10_000 });

    const displayed = await getVisibleTitleText(page);
    expect(displayed).toBe(newTitle);
  });

  test('after saving, clicking the title again re-enters edit mode', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('title-reedit');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    await waitForThreadTitle(page);

    // First edit cycle
    await clickVisibleElement(page, '.thread-title-display');
    let input = await waitForTitleInput(page);
    const firstTitle = `First ${Date.now()}`;
    await input.fill(firstTitle);
    await input.press('Enter');

    // Wait for the editor to leave edit mode (is-editing class removed)
    await page.waitForFunction(() => {
      return document.querySelectorAll('.thread-title-edit.is-editing').length === 0;
    }, undefined, { timeout: 10_000 });

    // Click the title again — without the fix, the still-focused overlay
    // input swallows the click and onFocus never re-fires, so editing
    // stays false and waitForTitleInput times out.
    await clickVisibleElement(page, '.thread-title-display');
    input = await waitForTitleInput(page);

    // Sanity: the input is the active element and editable.
    const isFocused = await page.evaluate(() => {
      const el = document.activeElement;
      return el?.tagName === 'INPUT' && el.classList.contains('thread-title-edit-input');
    });
    expect(isFocused).toBe(true);
  });

  test('press Escape cancels editing without saving', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('title-esc');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    await waitForThreadTitle(page);
    const originalTitle = await getVisibleTitleText(page);

    // Enter edit mode and type something different
    await clickVisibleElement(page, '.thread-title-display');
    const input = await waitForTitleInput(page);
    await input.fill('This should not be saved');
    await input.press('Escape');

    // Title should revert to original
    await page.waitForTimeout(500);
    const afterEscape = await getVisibleTitleText(page);
    expect(afterEscape).toBe(originalTitle);
  });
});

test.describe('Thread title editing — mobile', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('title appears in mobile header and can be edited', async ({ page }) => {
    await navigateToApp(page);

    // Send a message to create a thread
    const msg = uniqueMessage('mobile-title');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    // Wait for the title to appear in the mobile header
    await waitForThreadTitle(page);

    // Verify the title is inside the mobile title row (not the hidden desktop one)
    const titleInMobileRow = await page.evaluate(() => {
      const els = document.querySelectorAll('.thread-title-display');
      for (const el of els) {
        const rect = el.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          return !!el.closest('.mobile-thread-title-row');
        }
      }
      return false;
    });
    expect(titleInMobileRow).toBe(true);

    // Click the title to enter edit mode
    await clickVisibleElement(page, '.thread-title-display');
    const input = await waitForTitleInput(page);

    // Type a new title
    const newTitle = `Mobile Rename ${Date.now()}`;
    await input.fill(newTitle);
    await input.press('Enter');

    // Wait for the title display to update
    await page.waitForFunction((expected) => {
      const els = document.querySelectorAll('.thread-title-display');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && ((el as HTMLTextAreaElement).value ?? '').trim() === expected;
      });
    }, newTitle, { timeout: 10_000 });

    const displayed = await getVisibleTitleText(page);
    expect(displayed).toBe(newTitle);
  });

  test('edit input persists in DOM for synchronous focus (iOS keyboard fix)', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('ios-focus');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    // The edit <input> must exist in the DOM BEFORE clicking (not created on
    // click). This is required for iOS Safari to open the keyboard via the
    // synchronous focus() call inside the click handler on the display
    // textarea.
    const inputExistsBeforeClick = await page.evaluate(() => {
      const rows = document.querySelectorAll('.mobile-thread-title-row');
      for (const row of rows) {
        const rect = row.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          return !!row.querySelector('input.thread-title-edit-input');
        }
      }
      return false;
    });
    expect(inputExistsBeforeClick).toBe(true);

    // Click the display textarea — focus should jump to the edit <input>.
    await clickVisibleElement(page, '.thread-title-display');

    const editInputIsFocused = await page.evaluate(() => {
      const el = document.activeElement;
      return el?.tagName === 'INPUT' && el.classList.contains('thread-title-edit-input');
    });
    expect(editInputIsFocused).toBe(true);
  });

  test('edit input has user-select != none at focus time (iOS PWA keyboard fix)', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('user-select');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    // .mobile-thread-title-row sets user-select: none; the edit input must
    // override it (via CSS) so iOS PWA opens the keyboard on focus.
    await page.evaluate(() => {
      const el = document.querySelector(
        '.mobile-swipe-pane .mobile-thread-title-row input.thread-title-edit-input',
      );
      if (el) {
        el.addEventListener('focus', () => {
          const cs = getComputedStyle(el);
          (window as any).__userSelectAtFocus =
            cs.getPropertyValue('user-select') ||
            cs.getPropertyValue('-webkit-user-select');
        }, { once: true });
      }
    });

    // The transparent edit input is intentionally overlaid on the display
    // textarea so iOS PWA opens the keyboard on native focus. The overlay
    // intercepts real pointer events on the row, so use force:true to
    // dispatch the click on the display — its onClick handler calls
    // inputRef.focus(), firing the listener above.
    await page.locator('.mobile-swipe-pane .mobile-thread-title-row .thread-title-display').click({ force: true });
    await waitForTitleInput(page);

    const userSelectAtFocus = await page.evaluate(() => (window as any).__userSelectAtFocus);
    expect(userSelectAtFocus).toBeDefined();
    expect(userSelectAtFocus).not.toBe('none');
  });

  test('title display retains full height after rename (SSE race)', async ({ page }) => {
    // Repro for: editing the title on mobile and saving causes the title
    // display to collapse to ~2px (border height) — title appears to "disappear"
    // until reload restores it.
    //
    // Race: SSE delivers ThreadTitleRenamed before the rename HTTP response
    // resolves. The new title flows into ThreadTitleEditor via the threadMap
    // signal; its [title]-keyed useEffect runs autoResizeTextarea against the
    // display textarea while editing is still true (display:none → scrollHeight
    // 0 → style.height pinned to ~2px). When setEditing(false) finally runs,
    // the display becomes visible at the broken 2px height and never recomputes.
    //
    // We force the race by delaying /api/threads/rename so SSE always wins.
    await page.route('**/api/threads/rename', async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 500));
      await route.continue();
    });

    await navigateToApp(page);

    const msg = uniqueMessage('mobile-rename-race');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    // Capture pre-edit height as the baseline for "natural" height.
    const baselineHeight = await getMobileTitleHeight(page);
    expect(baselineHeight).toBeGreaterThan(10);

    await clickVisibleElement(page, '.thread-title-display');
    const input = await waitForTitleInput(page);
    const newTitle = `Race Rename ${Date.now()}`;
    await input.fill(newTitle);
    await input.press('Enter');

    // Wait for the new title to be reflected in the visible display.
    await page.waitForFunction((expected) => {
      const els = document.querySelectorAll('.thread-title-display');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && ((el as HTMLTextAreaElement).value ?? '').trim() === expected;
      });
    }, newTitle, { timeout: 10_000 });

    // The bug: the display's height collapses to the border-only height
    // (~2px). Assert the rendered height is at least most of the baseline.
    const postSaveHeight = await getMobileTitleHeight(page);
    expect(postSaveHeight).toBeGreaterThanOrEqual(baselineHeight - 1);
  });

  test('title hides with header on scroll down', async ({ page }) => {
    await navigateToApp(page);

    // Send multiple messages to create scrollable content
    const msg = uniqueMessage('scroll-title');
    await sendMessage(page, `Say exactly: "${msg}" and then write a very long paragraph with at least 200 words about anything`);
    await waitForResponse(page);

    await waitForThreadTitle(page);

    // Verify title is visible initially
    const titleVisibleBefore = await page.evaluate(() => {
      const els = document.querySelectorAll('.thread-title-display');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    });
    expect(titleVisibleBefore).toBe(true);

    // Blur any focused input — useHideOnScroll skips header hide when a text
    // input in the same pane is focused (prevents hide while user types).
    await blurActiveElement(page);

    // Inject extra content to guarantee the container is scrollable, since LLM
    // output length is unpredictable and may be too short on a small viewport.
    await page.evaluate(() => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      if (container) {
        const filler = document.createElement('div');
        // Use min-height + flex-shrink:0 so the filler isn't collapsed by the
        // flex container — plain height gets shrunk to fit the flex layout.
        filler.style.minHeight = '2000px';
        filler.style.flexShrink = '0';
        filler.dataset.testFiller = 'true';
        container.appendChild(filler);
      }
    });

    // Wait until the container is scrollable (filler appended + layout settled)
    await page.waitForFunction(() => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      return container ? container.scrollHeight > container.clientHeight : false;
    }, undefined, { timeout: 5_000 });

    // Scroll down in the thread content to trigger header hide
    await page.evaluate(() => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      if (container) container.scrollTop = container.scrollHeight;
    });

    // Wait for header to hide (translateY should be negative)
    await page.waitForFunction(() => {
      const header = document.querySelector('.app-header');
      if (!header) return false;
      return header.getBoundingClientRect().bottom <= 0;
    }, undefined, { timeout: 5_000 });

    // The title bar is sticky inside the scroll container and scrolls out
    // together with the header. Verify it is off-screen after full scroll.
    const titleOffScreen = await page.evaluate(() => {
      const els = document.querySelectorAll('.mobile-thread-title-row');
      for (const el of els) {
        const rect = el.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          return rect.bottom <= 0;
        }
      }
      return true; // not rendered = off-screen
    });
    expect(titleOffScreen).toBe(true);
  });
});
