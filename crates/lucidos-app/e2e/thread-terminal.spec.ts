import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import {
  apiRequest, assertHealthy, focusedThreadId, navigateToApp, openSettingsView, sendMessage,
  uniqueMessage, waitForEventStream, waitForResponse,
} from './helpers';

const SWITCH = '/api/v1/preferences?key=terminal_enabled';
const TERMINAL_BUTTON = '.thread-terminal-button:visible';
const STATUS = '.terminal-view:visible [data-role="terminal-status"]';
const SCREEN = '.terminal-view:visible .terminal-screen';

async function clearSwitch(page: Page): Promise<void> {
  await apiRequest(page).delete(SWITCH);
}

async function startChatThread(page: Page): Promise<string> {
  await navigateToApp(page);
  await waitForEventStream(page);
  await sendMessage(page, `Say exactly: "${uniqueMessage('terminal')}"`);
  await waitForResponse(page);
  return focusedThreadId(page);
}

async function typeLine(page: Page, line: string): Promise<void> {
  await page.locator(SCREEN).click();
  await page.keyboard.type(line);
  await page.keyboard.press('Enter');
}

test.describe('the thread terminal', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test.afterEach(async ({ page }) => {
    await clearSwitch(page);
  });

  test('shows no entry point until the Developer switch is turned on', async ({ page }) => {
    await clearSwitch(page);
    await startChatThread(page);
    await expect(page.locator('.thread-view-header')).toBeVisible();
    await expect(page.locator(TERMINAL_BUTTON)).toHaveCount(0);

    await openSettingsView(page, 'developer');
    const toggle = page.locator('.settings-panel:visible [data-search-anchor="developer:terminal"] input[type="checkbox"]');
    await expect(toggle).not.toBeChecked();
    await toggle.click({ force: true });
    await page.getByRole('button', { name: 'Turn on' }).click();
    await expect(toggle).toBeChecked();
    await expect(page.locator(TERMINAL_BUTTON)).toHaveCount(1);
  });

  test('runs a command, survives a reload, and ends when the thread is archived', async ({ page }) => {
    const put = await apiRequest(page).put(SWITCH, { data: { value: 'true' } });
    expect(put.ok()).toBe(true);
    const threadId = await startChatThread(page);

    await page.locator(TERMINAL_BUTTON).click();
    await expect(page.locator(STATUS)).toContainText('1 device attached', { timeout: 20_000 });

    await typeLine(page, 'echo "marker-$((6*7))"; pwd');
    await expect(page.locator(SCREEN)).toContainText('marker-42', { timeout: 10_000 });

    await page.reload();
    await waitForEventStream(page);
    if (!(await page.locator(SCREEN).isVisible())) await page.locator(TERMINAL_BUTTON).click();
    await expect(page.locator(STATUS)).toContainText('attached', { timeout: 20_000 });
    await expect(page.locator(SCREEN)).toContainText('marker-42');

    const archived = await apiRequest(page).post('/api/v1/threads/archive', { data: { thread_id: threadId } });
    expect(archived.ok()).toBe(true);
    await expect(page.locator(STATUS)).toContainText('Shell ended.', { timeout: 20_000 });
    await expect(page.locator('.terminal-view:visible .terminal-exit-cause')).toContainText('archived');
  });

  test('turning the switch off ends an open pane', async ({ page }) => {
    const put = await apiRequest(page).put(SWITCH, { data: { value: 'true' } });
    expect(put.ok()).toBe(true);
    await startChatThread(page);
    await page.locator(TERMINAL_BUTTON).click();
    await expect(page.locator(STATUS)).toContainText('attached', { timeout: 20_000 });

    await clearSwitch(page);
    await expect(page.locator(STATUS)).toContainText('Terminal is off.', { timeout: 20_000 });
    await expect(page.getByRole('button', { name: 'Open Settings' })).toBeVisible();
  });
});
