import { test, expect, Page } from './fixtures';
import {
  apiRequest, assertHealthy, blurActiveElement, closeComposerFold, COMPOSER_FOLD_TRIGGER, codingAgentCommandsReady, composerControl, ensureOnThreadPane,
  getHeaderTop, navigateToApp, newThread, pickComposeDestination, pickDropdownOption,
  ensureSlashCommandsReported, openRungList, sendMessage, uniqueMessage, waitForActionPanel, waitForVisibleInput,
} from './helpers';
import { clearAllThreads } from './db-helpers';

function hasVisibleElement(page: Page, selector: string): Promise<boolean> {
  return page.evaluate((sel) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, selector);
}

function waitForVisible(page: Page, selector: string, timeout = 10_000) {
  return page.waitForFunction((sel) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, selector, { timeout });
}

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
    const prefReset = await apiRequest(page).put('/api/v1/preferences?key=coding_agent_default', {
      data: { value: 'claude-code' },
    });
    expect(prefReset.ok()).toBeTruthy();
  });

  test('Claude button visible in compose view when Claude mode toggled', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    expect(await hasVisibleElement(page, '.commands-btn')).toBe(false);

    await pickComposeDestination(page);
    await expect(await composerControl(page, '.commands-btn')).toBeVisible();
  });

  test('"/" opens control command dropdown in compose view', async ({ page }) => {
    await ensureSlashCommandsReported(page);
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await expect.poll(() => codingAgentCommandsReady(page), { timeout: 15_000 }).toBe(true);

    await typeSlash(page);
    await waitForVisible(page, '.control-dropdown');

    expect(await hasVisibleElement(page, '.control-item')).toBe(true);

    const input = await waitForVisibleInput(page);
    const value = await input.inputValue();
    expect(value).toBe('');
  });

  test('Codex Lucidos-source draft has no `/` button and posts coding_agent=codex', async ({ page }) => {
    const commandUrls: string[] = [];
    let sentBody: Record<string, unknown> | null = null;
    let chatRequestFulfilled = false;

    page.on('request', (req) => {
      const url = req.url();
      if (url.includes('/api/v1/claude-code/commands')) commandUrls.push(url);
      if (url.includes('/api/v1/chat/stream') && req.method() === 'POST') {
        try {
          sentBody = req.postDataJSON() as Record<string, unknown>;
        } catch {  }
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

    await openRungList(page);
    await page.keyboard.press('Escape');
    await expect.poll(() => commandUrls.some((u) => {
      const url = new URL(u);
      return url.searchParams.get('repo_id') === ''
        && url.searchParams.get('coding_agent') === 'codex';
    })).toBe(true);
    await expect(page.locator('.prompt-actions-row .commands-btn:visible')).toHaveCount(0);
    const more = page.locator(COMPOSER_FOLD_TRIGGER).first();
    if (await more.count() > 0) {
      await more.click();
      await expect(page.locator('.thread-overflow-menu')).toBeVisible();
      await expect(page.locator('.thread-overflow-menu .commands-btn')).toHaveCount(0);
      await closeComposerFold(page);
    }

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
      await apiRequest(page).post(`/api/v1/claude-code/stop?thread_id=${encodeURIComponent(sentBody.thread_id)}`);
    }
  });

  test.describe('mobile header stays visible around control menu', () => {
    test.use({ viewport: { width: 375, height: 812 } });

    test('header visible after opening and closing coding-agent commands menu', async ({ page }) => {
      await ensureSlashCommandsReported(page);
      await navigateToApp(page);
      await newThread(page);
      await pickComposeDestination(page);

      await expect.poll(() => codingAgentCommandsReady(page), { timeout: 15_000 }).toBe(true);

      await blurActiveElement(page);
      await page.waitForFunction(() => {
        const header = document.querySelector('.app-header');
        return header ? header.getBoundingClientRect().top >= 0 : false;
      }, undefined, { timeout: 5_000 });

      expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);

      await (await composerControl(page, '.commands-btn')).click();
      await waitForVisible(page, '.control-dropdown');
      await page.keyboard.press('Escape');

      await page.waitForFunction(() => {
        const els = document.querySelectorAll('.control-dropdown');
        return !Array.from(els).some(el => {
          const rect = el.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0;
        });
      }, undefined, { timeout: 5_000 });

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

    const msg = uniqueMessage('cc-slash');
    await sendMessage(page, `Say exactly: "hello ${msg}" and nothing else. Do not create any files.`);

    await waitForActionPanel(page, 'Archive', 120_000);
    await ensureOnThreadPane(page);

    await typeSlash(page);
    await waitForVisible(page, '.control-dropdown');

    expect(await hasVisibleElement(page, '.control-item')).toBe(true);
  });
});
