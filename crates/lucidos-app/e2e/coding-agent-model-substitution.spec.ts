import { fileURLToPath } from 'node:url';
import { test, expect } from './fixtures';
import {
  apiRequest, assertHealthy, navigateToApp, newThread, pickComposeDestination,
  sendMessageCapturingThreadId, sendMessage, waitForActionPanel, dismissCCSession,
  openRungList, readRungRows, pickRung, currentRungValue, splitRungValue, modelSubstitutions,
} from './helpers';
import { clearAllThreads, psql } from './db-helpers';
import { resolveModel } from './fake-claude-code/claude.mjs';

/**
 * An account may not run the pinned model Lucidos asks for. Claude Code then
 * runs another one, and the thread says so in one step. The thread moves to
 * the served model at the picked effort, and a follow-up turn adds no step.
 *
 * The spec points `coding_agent_claude_path` at the fake Claude Code, so it
 * holds on a local run as well as on GitHub. The learned refusal takes the
 * picked rung off this workspace's ladder, so a retry picks the next one.
 */
const FAKE_CLAUDE = fileURLToPath(new URL('./fake-claude-code/claude.mjs', import.meta.url));
const PREF = 'coding_agent_claude_path';
const RESTRICTED = 'FAKE_CLAUDE_CODE_RESTRICTED_MODELS';
const SERVED = resolveModel('default');
const SUBSTITUTION_STEP = 'this account cannot use';

/** A model id with a version: only a pinned pick can be substituted. */
const isPinned = (model: string) => /-\d/.test(model);
const base = (model: string) => model.replace(/\[1m\]$/, '').split('@')[0];

test.describe('a Claude Code model substitution', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
    const pref = await apiRequest(page).put(`/api/v1/preferences?key=${PREF}`, { data: { value: FAKE_CLAUDE } });
    expect(pref.ok(), `${PREF}=${FAKE_CLAUDE}`).toBe(true);
  });

  test.afterEach(async ({ page }) => {
    await apiRequest(page).delete(`/api/v1/env-vars?name=${RESTRICTED}`);
    psql(`DELETE FROM preferences WHERE key = '${PREF}'`);
  });

  test('shows one step naming both models, and moves the thread to the served model at the picked effort', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await openRungList(page);
    const picked = (await readRungRows(page))
      .find((r) => r.effort && isPinned(r.model) && base(r.model) !== base(SERVED));
    expect(picked, 'a pinned rung other than the served model').toBeTruthy();
    await pickRung(page, picked!.value);

    // The account may not run the picked model.
    const env = await apiRequest(page).post('/api/v1/env-vars', { data: { name: RESTRICTED, value: picked!.model } });
    expect((await env.json()).success, `${RESTRICTED}=${picked!.model}`).toBe(true);

    const threadId = await sendMessageCapturingThreadId(page, 'Say exactly: "substitution-first-ok"');
    await expect(page.locator('.response-content', { hasText: 'substitution-first-ok' }).last()).toBeVisible({ timeout: 90_000 });
    await waitForActionPanel(page, 'Archive', 120_000);

    const substitutions = modelSubstitutions(threadId);
    expect(substitutions).toHaveLength(1);
    expect(substitutions[0]).toMatchObject({ requested: picked!.model, served: SERVED, coding_agent: 'claude-code' });
    await expect(page.locator('.step-description', { hasText: SUBSTITUTION_STEP })).toHaveCount(1);

    await openRungList(page);
    const current = splitRungValue(await currentRungValue(page));
    await page.keyboard.press('Escape');
    expect(current.model).not.toBe(picked!.model);
    expect(current.effort).toBe(picked!.effort);

    // The follow-up asks for the served model, so nothing is substituted again.
    await sendMessage(page, 'Say exactly: "substitution-second-ok"');
    await expect(page.locator('.response-content', { hasText: 'substitution-second-ok' }).last()).toBeVisible({ timeout: 90_000 });
    expect(modelSubstitutions(threadId)).toHaveLength(1);
    await expect(page.locator('.step-description', { hasText: SUBSTITUTION_STEP })).toHaveCount(1);

    await dismissCCSession(page);
  });
});
