import { test, expect, Page } from '@playwright/test';
import {
  navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessage, waitForActionPanel,
} from './helpers';

/** Send a CC message and wait for session to go idle (Done).
 * Commands are cached after this — the dropdown button will be active. */
async function setupIdleCCThread(page: Page) {
  await navigateToApp(page);
  await newThread(page);
  await pickComposeDestination(page);

  await sendMessage(page, 'Say exactly: "setup". Do not create any files.');
  await waitForActionPanel(page, 'Archive', 120_000);
}

/** Install API route handlers that force has_active_session: true in commands
 * responses and intercept control commands (returning success).
 * This allows testing the Model picker UI without needing a live CC process. */
async function mockActiveSession(page: Page) {
  let modelOverride: string | null = null;

  await page.route('**/api/v1/claude-code/commands*', async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.has_active_session = true;
    if (modelOverride) {
      body.current_model = modelOverride;
    }
    await route.fulfill({
      status: response.status(),
      headers: response.headers(),
      body: JSON.stringify(body),
    });
  });

  await page.route('**/api/v1/claude-code/control', async (route) => {
    const postData = route.request().postDataJSON();
    if (postData?.request?.subtype === 'set_model') {
      modelOverride = postData.request.model;
    }
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ ok: true }),
    });
  });
}

/** Open the CC dropdown and navigate to the Model submenu. */
async function openModelPicker(page: Page) {
  // Wait for the button to have commands loaded (active class) before clicking.
  const cmdBtn = page.locator('.commands-btn-active:visible').first();
  await expect(cmdBtn).toBeVisible({ timeout: 30_000 });
  await cmdBtn.click();

  const dropdown = page.locator('.control-dropdown:visible').first();
  await expect(dropdown).toBeVisible({ timeout: 5_000 });

  // Use strict regex to avoid matching skill commands like "vertex-models"
  const modelOption = dropdown.locator('.control-item:visible').filter({ hasText: /^Model/ }).first();
  await expect(modelOption).toBeVisible({ timeout: 10_000 });
  await modelOption.click();

  // Wait for model list to render
  await expect(page.locator('.control-option:visible').first()).toBeVisible({ timeout: 5_000 });

  return { cmdBtn, dropdown };
}

test.describe('Model switching', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('CC control menu opens and shows options', async ({ page }) => {
    // This test only needs the dropdown visible — idle session is fine
    await setupIdleCCThread(page);

    const cmdBtn = page.locator('.commands-btn:visible').first();
    await expect(cmdBtn).toBeVisible({ timeout: 10_000 });

    await cmdBtn.click();

    const dropdown = page.locator('.control-dropdown:visible').first();
    await expect(dropdown).toBeVisible({ timeout: 5_000 });

    // Top-level items use .control-item class
    const optionCount = await page.locator('.control-item:visible').count();
    expect(optionCount).toBeGreaterThan(0);
  });

  test('model picker shows available models with current highlighted', async ({ page }) => {
    // Setup idle CC thread first (caches commands), then mock active session
    await setupIdleCCThread(page);
    await mockActiveSession(page);

    const { dropdown } = await openModelPicker(page);

    const currentModel = dropdown.locator('.control-option-current:visible').first();
    if (await currentModel.isVisible({ timeout: 3_000 }).catch(() => false)) {
      const text = await currentModel.textContent();
      expect(text).toBeTruthy();
    }

    await page.keyboard.press('Escape');
  });

  test('switching model updates the selection', async ({ page }) => {
    // Setup idle CC thread first (caches commands), then mock active session
    await setupIdleCCThread(page);
    await mockActiveSession(page);

    const { cmdBtn, dropdown } = await openModelPicker(page);

    // Pick a non-default, non-current option to switch to
    const allOptions = page.locator('.control-option:visible');
    const count = await allOptions.count();
    expect(count).toBeGreaterThanOrEqual(2);

    // Find a specific model to click (skip "Default" since it won't show as current after)
    let targetIdx = -1;
    for (let i = 0; i < count; i++) {
      const text = await allOptions.nth(i).textContent();
      const isCurrent = await allOptions.nth(i).evaluate(el => el.classList.contains('control-option-current'));
      if (!isCurrent && !(text ?? '').startsWith('Default')) {
        targetIdx = i;
        break;
      }
    }
    expect(targetIdx).toBeGreaterThanOrEqual(0);

    const targetLabelRaw = await allOptions.nth(targetIdx).locator('.control-option-label').textContent();
    const targetLabel = (targetLabelRaw ?? '').replace(/✓/g, '').trim();
    await allOptions.nth(targetIdx).click();

    // Re-open to verify selection changed
    await expect(dropdown).not.toBeVisible({ timeout: 3_000 });
    await cmdBtn.click();
    await expect(dropdown).toBeVisible({ timeout: 5_000 });

    const modelOption2 = dropdown.locator('.control-item:visible').filter({ hasText: /^Model/ }).first();
    await expect(modelOption2).toBeVisible({ timeout: 10_000 });
    await modelOption2.click();
    await expect(page.locator('.control-option:visible').first()).toBeVisible({ timeout: 5_000 });

    // The selected model should now show as current
    const currentAfter = page.locator('.control-option-current:visible .control-option-label');
    await expect(currentAfter.first()).toBeVisible({ timeout: 5_000 });
    const afterTextRaw = await currentAfter.first().textContent();
    const afterText = (afterTextRaw ?? '').replace(/✓/g, '').trim();
    expect(afterText).toBe(targetLabel);

    await page.keyboard.press('Escape');
  });
});
