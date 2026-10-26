import { fileURLToPath } from 'node:url';
import { test, expect } from './fixtures';
import { psql } from './db-helpers';
import {
  apiRequest, navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessage, sendMessageCapturingThreadId, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, openRungList, readRungRows, pickRung, currentRungValue,
  answerRungSwitchIfAsked,
} from './helpers';
import type { Page } from '@playwright/test';

/**
 * A started coding-agent thread's model and effort change between turns only
 * (ADR 0542). A pick at idle is recorded on the engine, and the next turn's
 * spawn runs on it, though the follow-up itself carries no rung. While a turn
 * runs the engine refuses a pick, and so does the rung pill.
 */

const FAKE_CLAUDE = fileURLToPath(new URL('./fake-claude-code/claude.mjs', import.meta.url));
const CLAUDE_PATH_PREF = 'coding_agent_claude_path';

/** Wait until the engine reports the picked rung. A pick waits out the
 *  finished turn's idle exit, and a follow-up sent before it lands would
 *  reach the old process. */
async function waitForRecordedRung(page: Page, threadId: string, picked: { model: string; effort: string | null }) {
  await expect(async () => {
    await answerRungSwitchIfAsked(page, 250);
    const data = await (await page.request.get(`/api/v1/claude-code/commands?thread_id=${threadId}`)).json();
    expect(baseModel(data.current_model)).toBe(baseModel(picked.model));
    if (picked.effort) expect(data.current_reasoning_effort).toBe(picked.effort);
  }).toPass({ timeout: 15_000 });
}

/** The base model id, with its `[1m]` context-window twin folded in. */
const baseModel = (model: string | null | undefined) => (model ?? '').replace(/\[1m\]$/, '');

test.describe('CC mid-session model/effort persistence', () => {
  // The fake answers each prompt the same way every run, and real Claude Code
  // does not: it may stop on a permission card for a command of its own.
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    const res = await apiRequest(page).put(`/api/v1/preferences?key=${CLAUDE_PATH_PREF}`, { data: { value: FAKE_CLAUDE } });
    expect(res.ok(), `${CLAUDE_PATH_PREF}=${FAKE_CLAUDE}`).toBe(true);
  });

  test.afterEach(() => {
    psql(`DELETE FROM preferences WHERE key = '${CLAUDE_PATH_PREF}'`);
  });

  test('reasoning effort set mid-session persists after idle and follow-up', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    // Start CC session with default settings
    const sentThreadId = await sendMessageCapturingThreadId(
      page, 'Say exactly: "effort-persistence-ok". Do not create any files.',
    );
    await waitForCCToStart(page, 60_000);

    // Wait for session to be active
    await waitForActiveSession(page, sentThreadId);

    // Wait for response to complete (CC goes idle)
    await waitForActionPanel(page, 'Archive', 120_000);

    // === Change reasoning effort after CC went idle ===
    // The pick is recorded on the engine, which the next spawn runs on. A
    // rung commits its model and effort together.
    const before = await (await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`)).json();
    await openRungList(page);
    const picked = (await readRungRows(page)).find((r) => !r.current && r.effort !== before.current_reasoning_effort);
    expect(picked).toBeTruthy();
    await pickRung(page, picked!.value);
    await waitForRecordedRung(page, sentThreadId, picked!);

    // Send a follow-up, which respawns on the recorded effort
    await sendMessage(page, 'Say exactly: "effort-still-picked". Do not create any files.');
    await waitForCCToStart(page, 60_000);

    // === Core assertion: the picked effort must hold after respawn ===
    await expect(async () => {
      const cmdResp = await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`);
      expect(cmdResp.ok()).toBeTruthy();
      const cmdData = await cmdResp.json();
      expect(cmdData.current_reasoning_effort).toBe(picked!.effort);
    }).toPass({ timeout: 30_000 });

    await waitForActionPanel(page, 'Archive', 120_000);

    // Verify the rung control checks the picked rung
    await openRungList(page);
    expect(await currentRungValue(page)).toBe(picked!.value);

    // Cleanup
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });

  test('model changed mid-session persists after idle and follow-up', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    // Start CC session with default model
    const sentThreadId = await sendMessageCapturingThreadId(
      page, 'Say exactly: "model-persistence-ok". Do not create any files.',
    );
    await waitForCCToStart(page, 60_000);

    await waitForActiveSession(page, sentThreadId);
    await waitForActionPanel(page, 'Archive', 120_000);

    // === Change model after CC went idle ===
    // The pick is recorded on the engine, which the next spawn runs on.
    const before = await (await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`)).json();
    await openRungList(page);
    const picked = (await readRungRows(page)).find(
      (r) => !r.current && baseModel(r.model) !== baseModel(before.current_model),
    );
    expect(picked).toBeTruthy();
    await pickRung(page, picked!.value);
    await waitForRecordedRung(page, sentThreadId, picked!);

    // Send a follow-up, which respawns on the recorded rung
    await sendMessage(page, 'Say exactly: "model-still-picked". Do not create any files.');
    await waitForCCToStart(page, 60_000);

    // === Core assertion: the picked model must hold after respawn ===
    await expect(async () => {
      const cmdResp = await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`);
      expect(cmdResp.ok()).toBeTruthy();
      const cmdData = await cmdResp.json();
      expect(cmdData.current_model).toBe(picked!.model);
      expect(cmdData.current_reasoning_effort).toBe(picked!.effort);
    }).toPass({ timeout: 30_000 });

    await waitForActionPanel(page, 'Archive', 120_000);

    // Verify the rung control checks the picked rung
    await openRungList(page);
    expect(await currentRungValue(page)).toBe(picked!.value);

    // Cleanup
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });

  // A running turn keeps its rung: the engine refuses a pick with a reason
  // the client tells apart, and the pill says why. The turn runs to its end
  // on the effort it started with.
  test('a pick mid-turn is refused, and the rung records again at idle', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const threadId = await sendMessageCapturingThreadId(
      page, 'Take 20 seconds. Report your reasoning effort. Do not create any files.',
    );
    const before = await waitForActiveSession(page, threadId);
    const picked = before.current_reasoning_effort === 'low' ? 'high' : 'low';

    // The list still opens mid-turn, but a pick says why and records nothing.
    const lockReason = page.locator('.toast:visible', { hasText: 'stay fixed while a turn runs' });
    await openRungList(page);
    const other = (await readRungRows(page)).find((r) => !r.current)!;
    await page.locator(`.control-dropdown .control-option[data-value="${other.value}"]`).click();
    await expect(lockReason.first()).toBeVisible({ timeout: 5_000 });

    const res = await apiRequest(page).post('/api/v1/claude-code/control', {
      data: { thread_id: threadId, request: { subtype: 'set_reasoning_effort', effort: picked } },
    });
    expect(res.status(), await res.text()).toBe(409);
    expect((await res.json()).reason).toBe('turn-running');

    // The turn ran at the effort it started with, and nothing restarted it.
    await waitForActionPanel(page, 'Archive', 120_000);
    const eventsOf = (type: string, field: string) => psql(
      `SELECT payload->>'${field}' FROM events WHERE thread_id = '${threadId}' AND event_type = '${type}' ORDER BY sequence`,
    ).split('\n').map((l) => l.trim()).filter(Boolean);
    const replies = eventsOf('ResponseGenerated', 'text');
    expect(replies).toHaveLength(1);
    expect(replies[0]).not.toBe(`effort: ${picked}`);
    expect(eventsOf('ResponseCanceled', 'cause')).toEqual([]);

    // Idle: the list opens as before.
    await openRungList(page);
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });
});
