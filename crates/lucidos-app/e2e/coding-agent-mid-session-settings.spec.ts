import { fileURLToPath } from 'node:url';
import { test, expect } from './fixtures';
import { psql } from './db-helpers';
import {
  apiRequest, navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessage, sendMessageCapturingThreadId, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, openRungList, readRungRows, pickRung, currentRungValue,
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

const FAKE_CLAUDE = fileURLToPath(new URL('./fake-claude-code/claude.mjs', import.meta.url));
const CLAUDE_PATH_PREF = 'coding_agent_claude_path';

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
    // Session is removed from agent_sessions when process exits, so the pick
    // stores the rung as a pending preference (codingAgentPendingModel and
    // codingAgentPendingReasoningEffort). The follow-up sendMessage carries
    // the effort as `reasoning_effort` in the request body, which the next CC
    // spawn picks up. A rung commits its model and effort together.
    const before = await (await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`)).json();
    await openRungList(page);
    const picked = (await readRungRows(page)).find((r) => !r.current && r.effort !== before.current_reasoning_effort);
    expect(picked).toBeTruthy();
    await pickRung(page, picked!.value);

    // Send follow-up to trigger respawn with pending effort applied
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
    // Session is removed from agent_sessions when process exits, so the pick
    // stores the rung as a pending preference (codingAgentPendingModel). The
    // follow-up sendMessage carries the model as `cc_model` in the request
    // body, which the next CC spawn picks up.
    const before = await (await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`)).json();
    await openRungList(page);
    const picked = (await readRungRows(page)).find(
      (r) => !r.current && baseModel(r.model) !== baseModel(before.current_model),
    );
    expect(picked).toBeTruthy();
    await pickRung(page, picked!.value);

    // Send follow-up, which triggers a respawn with the pending rung applied
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

  // Claude Code takes effort only at spawn, so a pick mid-turn restarts the
  // session: the turn ends as a redirect, and a resume runs at the new
  // effort. The fake reports the CLAUDE_CODE_EFFORT_LEVEL it was spawned
  // with, which is what proves the restarted process runs at the pick.
  test('an effort pick mid-turn restarts the session at the new effort', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const threadId = await sendMessageCapturingThreadId(
      page, 'Take 30 seconds. Report your reasoning effort. Do not create any files.',
    );
    const before = await waitForActiveSession(page, threadId);
    const picked = before.current_reasoning_effort === 'low' ? 'high' : 'low';

    // A session still starting refuses the pick, so retry until it is live.
    await expect(async () => {
      const res = await apiRequest(page).post('/api/v1/claude-code/control', {
        data: { thread_id: threadId, request: { subtype: 'set_reasoning_effort', effort: picked } },
      });
      expect(res.ok(), await res.text()).toBeTruthy();
      expect((await res.json()).takes_effect).toBe('now');
    }).toPass({ timeout: 20_000, intervals: [500, 1000] });

    const eventsOf = (type: string, field: string) => psql(
      `SELECT payload->>'${field}' FROM events WHERE thread_id = '${threadId}' AND event_type = '${type}' ORDER BY sequence`,
    ).split('\n').map((l) => l.trim()).filter(Boolean);

    // The interrupted turn is a redirect, never the user's Stop.
    await expect.poll(() => eventsOf('ResponseCanceled', 'cause'), { timeout: 60_000 }).toEqual(['effort_restart']);
    await expect.poll(() => eventsOf('ContinuationStarted', 'reason'), { timeout: 60_000 }).toEqual(['effort_restart']);
    // The resumed process runs at the pick, and so does its next call.
    await expect.poll(() => eventsOf('ResponseGenerated', 'text'), { timeout: 60_000 }).toEqual([`effort: ${picked}`]);
    expect(psql(
      `SELECT payload->>'reasoning_effort' FROM events WHERE thread_id = '${threadId}' AND event_type = 'ContextCaptured' `
      + `AND payload->>'parent_tool_use_id' IS NULL AND payload->'usage'->>'input_tokens' IS NOT NULL ORDER BY sequence DESC LIMIT 1`,
    )).toBe(picked);

    // One turn: the restart row inside the reply names the cause, and no
    // resume card opens a second turn.
    await expect(page.locator('.event-row[data-role="restart"] .event-row-state:visible'))
      .toHaveText('You changed effort', { timeout: 30_000 });
    await expect(page.locator('.event-row[data-role="resume"]:visible')).toHaveCount(0);
    await waitForActionPanel(page, 'Archive', 120_000);
    await dismissCCSession(page);
  });
});
