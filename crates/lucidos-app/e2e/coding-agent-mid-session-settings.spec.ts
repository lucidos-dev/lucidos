import { test, expect } from '@playwright/test';
import {
  navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessage, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, waitAndClick,
} from './helpers';

/**
 * Bug: changing model or reasoning effort MID-SESSION (after CC is running)
 * doesn't persist. When the CC process exits idle and respawns on follow-up,
 * the model reverts to the base name (e.g., opus[1m] → opus) and reasoning
 * effort reverts to the default.
 *
 * Root causes fixed:
 * 1. send_cc_control_request() didn't update cc_commands_cache
 * 2. Session idle exit didn't save model/effort to cache before removal
 * 3. Respawn didn't fall back to cache when cc_model=None
 */

test.describe('CC mid-session model/effort persistence', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('reasoning effort set mid-session persists after idle and follow-up', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    // Capture thread ID from the chat request
    let sentThreadId: string | null = null;
    page.on('request', (req) => {
      if (req.url().includes('/api/v1/chat/stream') && req.method() === 'POST') {
        try {
          const body = req.postDataJSON();
          sentThreadId = body?.thread_id ?? null;
        } catch { /* ignore */ }
      }
    });

    // Start CC session with default settings
    await sendMessage(page, 'Say exactly: "effort-persistence-ok". Do not create any files.');
    await waitForCCToStart(page, 60_000);
    expect(sentThreadId).toBeTruthy();

    // Wait for session to be active
    await waitForActiveSession(page, sentThreadId!);

    // Wait for response to complete (CC goes idle)
    await waitForActionPanel(page, 'Archive', 120_000);

    // === Change reasoning effort after CC went idle ===
    // Session is removed from agent_sessions when process exits, so the click
    // stores 'max' as a pending preference (codingAgentPendingReasoningEffort). The
    // follow-up sendMessage carries it as `reasoning_effort` in the request
    // body, which the next CC spawn picks up.
    await waitAndClick(page, '.commands-btn-active', undefined, 15_000);
    await waitAndClick(page, '.control-item', 'Reasoning');
    await waitAndClick(page, '.control-option', 'Max');

    // Send follow-up to trigger respawn with pending effort applied
    await sendMessage(page, 'Say exactly: "effort-still-max". Do not create any files.');
    await waitForCCToStart(page, 60_000);

    // === Core assertion: effort must still be 'max' after respawn ===
    await expect(async () => {
      const cmdResp = await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`);
      expect(cmdResp.ok()).toBeTruthy();
      const cmdData = await cmdResp.json();
      expect(cmdData.current_reasoning_effort).toBe('max');
    }).toPass({ timeout: 30_000 });

    await waitForActionPanel(page, 'Archive', 120_000);

    // Verify UI also shows 'Max' in the control menu
    await waitAndClick(page, '.commands-btn-active', undefined, 15_000);
    await waitAndClick(page, '.control-item', 'Reasoning');

    const currentEffort = page.locator('.control-option-current:visible').first();
    await expect(currentEffort).toBeVisible({ timeout: 5_000 });
    const effortLabel = await currentEffort.locator('.control-option-label').textContent();
    expect((effortLabel ?? '').replace(/✓/g, '').trim()).toBe('Max');

    // Cleanup
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });

  test('model changed mid-session persists after idle and follow-up', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    let sentThreadId: string | null = null;
    page.on('request', (req) => {
      if (req.url().includes('/api/v1/chat/stream') && req.method() === 'POST') {
        try {
          const body = req.postDataJSON();
          sentThreadId = body?.thread_id ?? null;
        } catch { /* ignore */ }
      }
    });

    // Start CC session with default model
    await sendMessage(page, 'Say exactly: "model-persistence-ok". Do not create any files.');
    await waitForCCToStart(page, 60_000);
    expect(sentThreadId).toBeTruthy();

    await waitForActiveSession(page, sentThreadId!);
    await waitForActionPanel(page, 'Archive', 120_000);

    // === Change model after CC went idle ===
    // Session is removed from agent_sessions when process exits, so the click
    // stores 'haiku' as a pending preference (codingAgentPendingModel). The follow-up
    // sendMessage carries it as `cc_model` in the request body, which the
    // next CC spawn picks up.
    await waitAndClick(page, '.commands-btn-active', undefined, 15_000);
    await waitAndClick(page, '.control-item', 'Model');
    await waitAndClick(page, '.control-option', 'Haiku');

    // Send follow-up — triggers respawn with pending model applied
    await sendMessage(page, 'Say exactly: "model-still-haiku". Do not create any files.');
    await waitForCCToStart(page, 60_000);

    // === Core assertion: model must still be 'haiku' after respawn ===
    await expect(async () => {
      const cmdResp = await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`);
      expect(cmdResp.ok()).toBeTruthy();
      const cmdData = await cmdResp.json();
      expect(cmdData.current_model).toBe('haiku');
    }).toPass({ timeout: 30_000 });

    await waitForActionPanel(page, 'Archive', 120_000);

    // Verify UI shows Haiku in the control menu
    await waitAndClick(page, '.commands-btn-active', undefined, 15_000);
    await waitAndClick(page, '.control-item', 'Model');

    const currentModel = page.locator('.control-option-current:visible').first();
    await expect(currentModel).toBeVisible({ timeout: 5_000 });
    const modelLabel = await currentModel.locator('.control-option-label').textContent();
    expect((modelLabel ?? '').replace(/✓/g, '').trim()).toBe('Haiku 4.5');

    // Cleanup
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });
});
