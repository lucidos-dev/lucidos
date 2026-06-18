import { test, expect, Page } from './fixtures';
import {
  navigateToApp, assertHealthy, pickComposeDestination, newThread,
  waitForVisibleInput, ensureOnThreadPane, blurActiveElement,
  clickVisibleElement, getHeaderTop,
  sendMessage, uniqueMessage, waitForActionPanel,
  pickDropdownOption,
} from './helpers';
import { clearAllThreads } from './db-helpers';

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

/** Type "/" into the visible prompt input, triggering the control menu open request.
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

test.describe('Coding-agent control menu', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
    const prefReset = await page.request.put('/api/v1/preferences?key=coding_agent_default', {
      data: { value: 'claude-code' },
    });
    expect(prefReset.ok()).toBeTruthy();
  });

  test('Claude button visible in compose view when Claude mode toggled', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    // In Lucidos mode — no coding-agent controls button. The Lucidos Agent
    // model picker shares the `commands-btn` base style class (distinguished
    // by `lucidos-commands-btn`), so exclude it to target only the control button.
    expect(await hasVisibleElement(page, '.commands-btn:not(.lucidos-commands-btn)')).toBe(false);

    // Toggle to Claude mode — control button appears
    await pickComposeDestination(page);
    expect(await hasVisibleElement(page, '.commands-btn:not(.lucidos-commands-btn)')).toBe(true);
  });

  test('"/" opens control command dropdown in compose view', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    // Wait for coding-agent commands to load (button gets 'active' class when ready)
    await waitForVisible(page, '.commands-btn-active', 15_000);

    // Type "/" — should open the control command dropdown
    await typeSlash(page);
    await waitForVisible(page, '.control-dropdown');

    // Verify dropdown has command items
    expect(await hasVisibleElement(page, '.control-item')).toBe(true);

    // The prompt input should be cleared (slash consumed)
    const input = await waitForVisibleInput(page);
    const value = await input.inputValue();
    expect(value).toBe('');
  });

  test('Codex Lucidos-source draft shows Codex controls and posts coding_agent=codex', async ({ page }) => {
    const commandUrls: string[] = [];
    let sentBody: Record<string, unknown> | null = null;
    let chatRequestFulfilled = false;

    // Chromium hits the route stub, keeping the test from spawning a real
    // backend session. Mobile WebKit can let this POST through, so the request
    // event is the assertion source and the route is a best-effort fast path.
    page.on('request', (req) => {
      const url = req.url();
      if (url.includes('/api/v1/claude-code/commands')) commandUrls.push(url);
      if (url.includes('/api/v1/chat/stream') && req.method() === 'POST') {
        try {
          sentBody = req.postDataJSON() as Record<string, unknown>;
        } catch { /* ignore */ }
      }
    });
    await page.route('**/api/v1/chat/stream', async (route) => {
      sentBody = route.request().postDataJSON() as Record<string, unknown>;
      chatRequestFulfilled = true;
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ event_id: sentBody.event_id }),
      });
    });

    await navigateToApp(page);
    await newThread(page);

    const input = await waitForVisibleInput(page);
    const msg = uniqueMessage('codex-compose');
    await input.fill(`Say exactly: "codex ${msg}" and nothing else. Do not create any files.`);
    await expect(input).toHaveAttribute('data-thread-id', /.+/, { timeout: 5_000 });

    await pickComposeDestination(page, 'Lucidos source');
    await pickDropdownOption(page, '.compose-coding-agent-chip', 'Codex');
    await expect(
      page.locator('.compose-coding-agent-chip .dropdown-sizer > :first-child:visible').first(),
    ).toHaveText('Codex');

    const codexControls = page.locator('.commands-btn-active[aria-label="Codex controls"]:visible').first();
    await expect(codexControls).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('.commands-btn[aria-label="Claude Code controls"]:visible')).toHaveCount(0);
    await expect.poll(() => commandUrls.some((u) => {
      const url = new URL(u);
      return url.searchParams.get('repo_id') === ''
        && url.searchParams.get('coding_agent') === 'codex';
    })).toBe(true);

    await codexControls.click();
    await waitForVisible(page, '.control-dropdown');
    await expect(page.locator('.control-section-label:visible', { hasText: 'Session' }).first()).toBeVisible();
    await expect(page.locator('.control-section-label:visible', { hasText: 'Commands' })).toHaveCount(0);
    await expect(page.locator('.control-section-label:visible', { hasText: 'Skills' })).toHaveCount(0);
    await codexControls.click();

    await typeSlash(page);
    await page.waitForTimeout(250);
    await expect(page.locator('.control-dropdown:visible')).toHaveCount(0);
    await expect(await waitForVisibleInput(page)).toHaveValue('/');

    await sendMessage(page, `Say exactly: "codex ${msg}" and nothing else. Do not create any files.`);
    await expect.poll(() => sentBody?.coding_agent ?? null, { timeout: 5_000 }).toBe('codex');
    expect(sentBody?.use_coding_agent).toBe(true);
    expect(sentBody?.folder).toBeUndefined();
    expect(sentBody?.repo_id).toBeUndefined();
    if (!chatRequestFulfilled && typeof sentBody?.thread_id === 'string') {
      await page.request.post(`/api/v1/claude-code/stop?thread_id=${encodeURIComponent(sentBody.thread_id)}`);
    }
  });

  test.describe('mobile header stays visible around control menu', () => {
    test.use({ viewport: { width: 375, height: 812 } });

    test('header visible after opening and closing coding-agent commands menu', async ({ page }) => {
      await navigateToApp(page);
      await newThread(page);
      await pickComposeDestination(page);

      // Wait for coding-agent commands to load
      await waitForVisible(page, '.commands-btn-active', 15_000);

      // Blur any auto-focused element so header is visible
      await blurActiveElement(page);
      await page.waitForFunction(() => {
        const header = document.querySelector('.app-header');
        return header ? header.getBoundingClientRect().top >= 0 : false;
      }, undefined, { timeout: 5_000 });

      expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);

      // Open the coding-agent commands dropdown via button click
      await clickVisibleElement(page, '.commands-btn');
      await waitForVisible(page, '.control-dropdown');

      // Close the dropdown by clicking the button again
      await clickVisibleElement(page, '.commands-btn');

      // Wait for dropdown to disappear
      await page.waitForFunction(() => {
        const els = document.querySelectorAll('.control-dropdown');
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

  test('"/" opens control command dropdown in existing coding-agent thread', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    // Start a coding-agent session
    const msg = uniqueMessage('cc-slash');
    await sendMessage(page, `Say exactly: "hello ${msg}" and nothing else. Do not create any files.`);

    // Wait for session to finish and idle
    await waitForActionPanel(page, 'Archive', 120_000);
    await ensureOnThreadPane(page);

    // Now type "/" in the existing coding-agent thread — should open command menu
    await typeSlash(page);
    await waitForVisible(page, '.control-dropdown');

    // Verify dropdown has command items
    expect(await hasVisibleElement(page, '.control-item')).toBe(true);
  });
});
