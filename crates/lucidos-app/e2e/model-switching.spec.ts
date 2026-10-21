import { test, expect, Page } from './fixtures';
import {
  navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessage, waitForActionPanel, openRungList, readRungRows, pickRung, currentRungValue,
  splitRungValue,
} from './helpers';

// `sw.js` serves the commands and estimate GETs. Once the worker controls the
// page, those fetches run in the worker, where `page.route` cannot see them.
// The mocks below then lapse.
test.use({ serviceWorkers: 'block' });

/** Send a CC message and wait for session to go idle (Done).
 * Commands are cached after this, so the rung control has its rungs. */
async function setupIdleCCThread(page: Page) {
  await navigateToApp(page);
  await newThread(page);
  await pickComposeDestination(page);

  await sendMessage(page, 'Say exactly: "setup". Do not create any files.');
  await waitForActionPanel(page, 'Archive', 120_000);
}

/** Install API route handlers that force has_active_session: true in commands
 * responses and intercept control commands (returning success).
 * This allows testing the rung control without needing a live CC process.
 * Call it BEFORE the thread is set up. The app reads the commands again only on
 * a session event. A mock installed after the last event never reaches it, so
 * the app holds `has_active_session: false`.
 * Returns the control requests the app sent, in order. */
async function mockActiveSession(page: Page) {
  let modelOverride: string | null = null;
  let effortOverride: string | null = null;
  const controls: Array<{ subtype?: string; model?: string; effort?: string }> = [];

  await page.route('**/api/v1/claude-code/commands*', async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.has_active_session = true;
    if (modelOverride) body.current_model = modelOverride;
    if (effortOverride) body.current_reasoning_effort = effortOverride;
    await route.fulfill({
      status: response.status(),
      headers: response.headers(),
      body: JSON.stringify(body),
    });
  });

  // A pick on a live thread asks what it costs first. This one is free.
  await page.route(/\/api\/v1\/threads\/[^/]+\/switch-estimate/, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ context_tokens: 0, rebuilds_cache: false, extra_cost_usd: null }),
    });
  });

  // The request event is the record of what the app sent.
  page.on('request', (req) => {
    if (req.method() !== 'POST' || !req.url().endsWith('/api/v1/claude-code/control')) return;
    try {
      controls.push(req.postDataJSON()?.request ?? {});
    } catch { /* a body that is not JSON is no control request */ }
  });

  await page.route('**/api/v1/claude-code/control', async (route) => {
    const request = route.request().postDataJSON()?.request ?? {};
    if (request.subtype === 'set_model') modelOverride = request.model;
    if (request.subtype === 'set_reasoning_effort') effortOverride = request.effort;
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ ok: true }),
    });
  });

  return controls;
}

test.describe('Model switching', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  // A refetch can still be in flight when the test ends. Playwright then
  // disposes its response while the context closes, before `page.isClosed()`
  // turns true. Dropping the routes first ignores that handler's error.
  test.afterEach(async ({ page }) => {
    if (!page.isClosed()) await page.unrouteAll({ behavior: 'ignoreErrors' });
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

  test('rung list shows the ladder with the current rung checked', async ({ page }) => {
    await mockActiveSession(page);
    await setupIdleCCThread(page);

    const dropdown = await openRungList(page);

    // One row is checked: a rung, or the thread's own pair.
    const currentModel = dropdown.locator('.control-option-current:visible');
    await expect(currentModel).toHaveCount(1);
    expect((await currentModel.textContent())?.trim()).toBeTruthy();

    // The list carries rungs and the ladder link. No rung opens a tier step.
    expect((await readRungRows(page)).length).toBeGreaterThan(0);
    await expect(dropdown.locator('.control-option[data-value="ladder-link"]:visible')).toBeVisible();
    await expect(dropdown.locator('.control-option:not([data-value="ladder-link"]) .control-option-more')).toHaveCount(0);

    await page.keyboard.press('Escape');
  });

  test('ending the test mid-refetch does not fail the commands mock', async ({ page }) => {
    await mockActiveSession(page);
    await navigateToApp(page);

    // Leave a commands fetch in flight and end the test under it.
    await page.evaluate(() => { void fetch('/api/v1/claude-code/commands').catch(() => {}); });
  });

  test('Escape closes the rung list', async ({ page }) => {
    await mockActiveSession(page);
    await setupIdleCCThread(page);

    const dropdown = await openRungList(page);
    await page.keyboard.press('Escape');
    await expect(dropdown).not.toBeVisible({ timeout: 3_000 });
  });

  test('picking a rung sends its model and effort and updates the selection', async ({ page }) => {
    const controls = await mockActiveSession(page);
    await setupIdleCCThread(page);

    await openRungList(page);

    // Pick a rung that is not the checked one. One tap commits both halves.
    const rungs = await readRungRows(page);
    expect(rungs.length).toBeGreaterThanOrEqual(2);
    const target = rungs.find((r) => !r.current);
    expect(target).toBeTruthy();
    await pickRung(page, target!.value);

    await expect.poll(() => controls.map((c) => c.subtype)).toContain('set_model');
    expect(controls.find((c) => c.subtype === 'set_model')?.model).toBe(target!.model);
    if (target!.effort) {
      await expect.poll(() => controls.find((c) => c.subtype === 'set_reasoning_effort')?.effort)
        .toBe(target!.effort);
    }

    // Re-open to verify selection changed
    await openRungList(page);
    const checked = await currentRungValue(page);
    expect(splitRungValue(checked)).toEqual({ model: target!.model, effort: target!.effort });

    await page.keyboard.press('Escape');
  });
});
