import { test, expect, Page } from '@playwright/test';
import {
  navigateToApp, assertHealthy, switchToClaudeMode, newThread,
  waitForVisibleInput, ensureOnThreadPane, blurActiveElement,
  clickVisibleElement, getHeaderTop,
  sendMessage, uniqueMessage, waitForActionPanel,
} from './helpers';

/** Check if any element matching the selector is physically visible (dual-layout safe). */
function hasVisibleElement(page: Page, selector: string): Promise<boolean> {
  return page.evaluate((sel) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, selector);
}

/** Wait for at least one element matching the selector to be physically visible. */
function waitForVisible(page: Page, selector: string, timeout = 10_000) {
  return page.waitForFunction((sel) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, selector, { timeout });
}

/** Type "/" into the visible prompt input, triggering the CC menu open request.
 *  Uses evaluate + dispatchEvent because Playwright's fill() doesn't trigger
 *  the handleInput code path that detects the "/" prefix. */
async function typeSlash(page: Page) {
  await page.evaluate(() => {
    const els = document.querySelectorAll('[data-role="prompt-input"]');
    for (const el of els) {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        const textarea = el as HTMLTextAreaElement;
        textarea.focus();
        textarea.value = '/';
        textarea.dispatchEvent(new Event('input', { bubbles: true }));
        return;
      }
    }
  });
}

test.describe('CC slash command menu', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('Claude button visible in compose view when Claude mode toggled', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    // In Lucidos mode — no CC button
    expect(await hasVisibleElement(page, '.cc-commands-btn')).toBe(false);

    // Toggle to Claude mode — CC button appears
    await switchToClaudeMode(page);
    expect(await hasVisibleElement(page, '.cc-commands-btn')).toBe(true);
  });

  test('"/" opens CC command dropdown in compose view', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    // Wait for CC commands to load (button gets 'active' class when ready)
    await waitForVisible(page, '.cc-commands-btn-active', 15_000);

    // Type "/" — should open the CC command dropdown
    await typeSlash(page);
    await waitForVisible(page, '.cc-control-dropdown');

    // Verify dropdown has command items
    expect(await hasVisibleElement(page, '.cc-control-item')).toBe(true);

    // The prompt input should be cleared (slash consumed)
    const input = await waitForVisibleInput(page);
    const value = await input.inputValue();
    expect(value).toBe('');
  });

  test.describe('mobile header stays visible around CC menu', () => {
    test.use({ viewport: { width: 375, height: 812 } });

    test('header visible after opening and closing CC commands menu', async ({ page }) => {
      await navigateToApp(page);
      await newThread(page);
      await switchToClaudeMode(page);

      // Wait for CC commands to load
      await waitForVisible(page, '.cc-commands-btn-active', 15_000);

      // Blur any auto-focused element so header is visible
      await blurActiveElement(page);
      await page.waitForFunction(() => {
        const header = document.querySelector('.app-header');
        return header ? header.getBoundingClientRect().top >= 0 : false;
      }, undefined, { timeout: 5_000 });

      expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);

      // Open the CC commands dropdown via button click
      await clickVisibleElement(page, '.cc-commands-btn');
      await waitForVisible(page, '.cc-control-dropdown');

      // Close the dropdown by clicking the button again
      await clickVisibleElement(page, '.cc-commands-btn');

      // Wait for dropdown to disappear
      await page.waitForFunction(() => {
        const els = document.querySelectorAll('.cc-control-dropdown');
        return !Array.from(els).some(el => {
          const rect = el.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0;
        });
      }, undefined, { timeout: 5_000 });

      // Header must be visible again after closing the menu
      await page.waitForFunction(() => {
        const header = document.querySelector('.app-header');
        return header ? header.getBoundingClientRect().top >= 0 : false;
      }, undefined, { timeout: 5_000 });

      expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);
    });
  });

  test('"/" opens CC command dropdown in existing CC thread', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    // Start a CC session
    const msg = uniqueMessage('cc-slash');
    await sendMessage(page, `Say exactly: "hello ${msg}" and nothing else. Do not create any files.`);

    // Wait for session to finish and idle
    await waitForActionPanel(page, 'Archive', 120_000);
    await ensureOnThreadPane(page);

    // Now type "/" in the existing CC thread — should open command menu
    await typeSlash(page);
    await waitForVisible(page, '.cc-control-dropdown');

    // Verify dropdown has command items
    expect(await hasVisibleElement(page, '.cc-control-item')).toBe(true);
  });
});
