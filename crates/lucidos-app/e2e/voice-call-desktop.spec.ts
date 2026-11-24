import { test, expect } from './fixtures';
import { apiRequest, gotoWithRetry, hideDashboardRail, newThread, setVoiceEnabled, waitForVisibleInput } from './helpers';
import { ensureHomeThread } from './db-helpers';
import { stampPageWebSockets } from './harnessStamp';

test.use({
  launchOptions: {
    args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'],
  },
  permissions: ['microphone'],
});

const OPENAI_SWITCH_KEY = 'provider_enabled_openai';
const CALL_TOGGLE = '[data-role="call-toggle"]';

async function setOpenAiProvider(
  page: import('@playwright/test').Page,
  on: boolean,
): Promise<void> {
  const res = await apiRequest(page).put(`/api/v1/preferences?key=${OPENAI_SWITCH_KEY}`, {
    data: { value: String(on) },
  });
  expect(res.ok()).toBeTruthy();
}

test.describe('a call the engine cannot take', () => {
  test.beforeEach(async ({ page }) => {
    await hideDashboardRail(page);
    await stampPageWebSockets(page, /\/api\/v1\/voice\?/);
    await page.addInitScript(() => {
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('lucidos-split-ratio', '1');
    });
  });

  test.beforeAll(async ({ browser }) => {
    const page = await browser.newPage();
    await setVoiceEnabled(page, true);
    await setOpenAiProvider(page, false);
    await page.close();
  });

  test.afterAll(async ({ browser }) => {
    const page = await browser.newPage();
    await setVoiceEnabled(page, false);
    await setOpenAiProvider(page, true);
    await page.close();
  });

  test('says why in plain English, and leaves no call behind', async ({ page }) => {
    await gotoWithRetry(page, `/#thread=${ensureHomeThread()}`);
    await waitForVisibleInput(page);

    const toggle = page.locator(CALL_TOGGLE).first();
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');

    await toggle.click();

    const toast = page.locator('.toast-error:visible').first();
    await expect(toast).toBeVisible({ timeout: 30_000 });
    await expect(toast).toContainText('voice model');

    const said = (await toast.textContent()) ?? '';
    expect(said).not.toMatch(/openai|gpt-|realtime|\b[45]\d\d\b/i);

    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
    await expect(page.locator('[data-role="call-state"]').first()).toBeEmpty();
  });

  test('the toggle is offered on the home thread, and not in the compose view', async ({ page }) => {
    await gotoWithRetry(page, `/#thread=${ensureHomeThread()}`);
    await waitForVisibleInput(page);
    const toggle = page.locator(CALL_TOGGLE).first();
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAccessibleName('Start a call');

    await newThread(page);
    await waitForVisibleInput(page);
    await expect(page.locator(`${CALL_TOGGLE}:visible`)).toHaveCount(0);
  });
});
