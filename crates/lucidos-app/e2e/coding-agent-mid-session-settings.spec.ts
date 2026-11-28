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


const FAKE_CLAUDE = fileURLToPath(new URL('./fake-claude-code/claude.mjs', import.meta.url));
const CLAUDE_PATH_PREF = 'coding_agent_claude_path';

async function waitForRecordedRung(page: Page, threadId: string, picked: { model: string; effort: string | null }) {
  await expect(async () => {
    await answerRungSwitchIfAsked(page, 250);
    const data = await (await page.request.get(`/api/v1/claude-code/commands?thread_id=${threadId}`)).json();
    expect(baseModel(data.current_model)).toBe(baseModel(picked.model));
    if (picked.effort) expect(data.current_reasoning_effort).toBe(picked.effort);
  }).toPass({ timeout: 15_000 });
}

const baseModel = (model: string | null | undefined) => (model ?? '').replace(/\[1m\]$/, '');

test.describe('CC mid-session model/effort persistence', () => {
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

    const sentThreadId = await sendMessageCapturingThreadId(
      page, 'Say exactly: "effort-persistence-ok". Do not create any files.',
    );
    await waitForCCToStart(page, 60_000);

    await waitForActiveSession(page, sentThreadId);

    await waitForActionPanel(page, 'Archive', 120_000);

    const before = await (await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`)).json();
    await openRungList(page);
    const picked = (await readRungRows(page)).find((r) => !r.current && r.effort !== before.current_reasoning_effort);
    expect(picked).toBeTruthy();
    await pickRung(page, picked!.value);
    await waitForRecordedRung(page, sentThreadId, picked!);

    await sendMessage(page, 'Say exactly: "effort-still-picked". Do not create any files.');
    await waitForCCToStart(page, 60_000);

    await expect(async () => {
      const cmdResp = await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`);
      expect(cmdResp.ok()).toBeTruthy();
      const cmdData = await cmdResp.json();
      expect(cmdData.current_reasoning_effort).toBe(picked!.effort);
    }).toPass({ timeout: 30_000 });

    await waitForActionPanel(page, 'Archive', 120_000);

    await openRungList(page);
    expect(await currentRungValue(page)).toBe(picked!.value);

    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });

  test('model changed mid-session persists after idle and follow-up', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const sentThreadId = await sendMessageCapturingThreadId(
      page, 'Say exactly: "model-persistence-ok". Do not create any files.',
    );
    await waitForCCToStart(page, 60_000);

    await waitForActiveSession(page, sentThreadId);
    await waitForActionPanel(page, 'Archive', 120_000);

    const before = await (await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`)).json();
    await openRungList(page);
    const picked = (await readRungRows(page)).find(
      (r) => !r.current && baseModel(r.model) !== baseModel(before.current_model),
    );
    expect(picked).toBeTruthy();
    await pickRung(page, picked!.value);
    await waitForRecordedRung(page, sentThreadId, picked!);

    await sendMessage(page, 'Say exactly: "model-still-picked". Do not create any files.');
    await waitForCCToStart(page, 60_000);

    await expect(async () => {
      const cmdResp = await page.request.get(`/api/v1/claude-code/commands?thread_id=${sentThreadId}`);
      expect(cmdResp.ok()).toBeTruthy();
      const cmdData = await cmdResp.json();
      expect(cmdData.current_model).toBe(picked!.model);
      expect(cmdData.current_reasoning_effort).toBe(picked!.effort);
    }).toPass({ timeout: 30_000 });

    await waitForActionPanel(page, 'Archive', 120_000);

    await openRungList(page);
    expect(await currentRungValue(page)).toBe(picked!.value);

    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });

  test('a pick mid-turn is refused, and the rung records again at idle', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const threadId = await sendMessageCapturingThreadId(
      page, 'Take 20 seconds. Report your reasoning effort. Do not create any files.',
    );
    const before = await waitForActiveSession(page, threadId);
    const picked = before.current_reasoning_effort === 'low' ? 'high' : 'low';

    const trigger = page.locator('.rung-control:visible .rung-trigger');
    await expect(trigger).toHaveAttribute('aria-disabled', 'false');
    await expect(trigger).toHaveAttribute('data-tooltip', /stay fixed while a turn runs/);
    await trigger.click();
    await expect(page.locator('.control-dropdown:visible')).toBeVisible();
    await page.keyboard.press('Escape');

    const res = await apiRequest(page).post('/api/v1/claude-code/control', {
      data: { thread_id: threadId, request: { subtype: 'set_reasoning_effort', effort: picked } },
    });
    expect(res.status(), await res.text()).toBe(409);
    expect((await res.json()).reason).toBe('turn-running');

    await waitForActionPanel(page, 'Archive', 120_000);
    const eventsOf = (type: string, field: string) => psql(
      `SELECT payload->>'${field}' FROM events WHERE thread_id = '${threadId}' AND event_type = '${type}' ORDER BY sequence`,
    ).split('\n').map((l) => l.trim()).filter(Boolean);
    const replies = eventsOf('ResponseGenerated', 'text');
    expect(replies).toHaveLength(1);
    expect(replies[0]).not.toBe(`effort: ${picked}`);
    expect(eventsOf('ResponseCanceled', 'cause')).toEqual([]);

    await openRungList(page);
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });
});
