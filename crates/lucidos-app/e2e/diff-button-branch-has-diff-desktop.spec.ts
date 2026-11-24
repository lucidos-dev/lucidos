import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, hideDashboardRail } from './helpers';
import {
  psql, git, WORKSPACE, cleanupCCThread, cleanupFileFromMain,
} from './db-helpers';
import { writeFileSync } from 'fs';
import { resolve } from 'path';
import { randomUUID } from 'crypto';
import type { Page } from './fixtures';


async function diffVisible(page: Page): Promise<boolean> {
  return page.locator('.thread-action-buttons:visible button[data-role="thread-diff"]').first()
    .isVisible().catch(() => false);
}

function seedWaitingThread(suffix: string): { threadId: string } {
  const threadId = randomUUID();
  const now = new Date().toISOString();

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'E2E Diff Branch ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'inbox', true, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"test","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Done.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'CodingAgentIdled', '{"has_changes":false,"is_external_repo":false,"requires_restart":false}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));

  return { threadId };
}

function createBranch(suffix: string): { branch: string; file: string } {
  const branch = `e2e-test/diff-${suffix}`;
  const file = `e2e-diff-${suffix}.txt`;
  git(['checkout', '-b', branch, 'main']);
  writeFileSync(resolve(WORKSPACE, file), `diff content ${suffix}`);
  git(['add', '.']);
  git(['commit', '-m', `e2e diff ${suffix}`]);
  git(['checkout', 'main']);
  return { branch, file };
}

async function seedChangeProposed(page: Page, opts: {
  changeId: string;
  threadId: string;
  branch: string;
  file: string;
  description: string;
}): Promise<void> {
  const resp = await apiRequest(page).post('/api/v1/internal/seed-change-for-test', {
    data: {
      change_id: opts.changeId,
      thread_id: opts.threadId,
      branch_name: opts.branch,
      repo_root: WORKSPACE,
      description: opts.description,
      files: [opts.file],
      requires_restart: false,
      hardened: true,
      needs_hardening: false,
      apply_ready: true,
    },
  });
  expect(resp.ok(), `seed-change-for-test failed: ${resp.status()} ${await resp.text()}`).toBeTruthy();
}

test.describe('WaitingBanner Diff button reacts to coding_agent_change_state', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    psql("UPDATE thread_summaries SET status = 'idle', archive_state = 'archived' WHERE status = 'waiting' AND is_coding_agent = true");
  });

  function archiveButton(page: Page) {
    return page.locator('.thread-action-buttons:visible button:has-text("Archive")').first();
  }

  test('CC thread with no diff: Diff button is hidden', async ({ page }) => {
    const suffix = `noop-${Date.now()}`;
    const { threadId } = seedWaitingThread(suffix);

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await hideDashboardRail(page);
      await navigateToApp(page);

      await expect(archiveButton(page)).toBeVisible({ timeout: 15_000 });
      expect(await diffVisible(page)).toBe(false);
    } finally {
      cleanupCCThread(threadId);
    }
  });

  test('After ChangeProposed lands, the Diff button appears', async ({ page }) => {
    const suffix = `enable-${Date.now()}`;
    const { threadId } = seedWaitingThread(suffix);
    const { branch, file } = createBranch(suffix);
    const changeId = randomUUID();

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await hideDashboardRail(page);
      await navigateToApp(page);

      await expect(archiveButton(page)).toBeVisible({ timeout: 15_000 });
      expect(await diffVisible(page)).toBe(false);

      await seedChangeProposed(page, {
        changeId, threadId, branch, file,
        description: `E2E diff enable ${suffix}`,
      });

      await expect.poll(() => diffVisible(page), { timeout: 10_000 }).toBe(true);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('After ChangeApplied lands, the Diff button disappears', async ({ page }) => {
    const suffix = `disable-${Date.now()}`;
    const { threadId } = seedWaitingThread(suffix);
    const { branch, file } = createBranch(suffix);
    const changeId = randomUUID();
    let appliedToMain = false;

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await hideDashboardRail(page);
      await navigateToApp(page);

      await expect(archiveButton(page)).toBeVisible({ timeout: 15_000 });

      await seedChangeProposed(page, {
        changeId, threadId, branch, file,
        description: `E2E diff disable ${suffix}`,
      });
      await expect.poll(() => diffVisible(page), { timeout: 10_000 }).toBe(true);

      const applyResp = await apiRequest(page).post(`/api/v1/changes/${changeId}/apply`);
      expect(applyResp.ok(), `apply failed: ${applyResp.status()} ${await applyResp.text()}`).toBeTruthy();
      appliedToMain = true;

      await expect.poll(() => diffVisible(page), { timeout: 10_000 }).toBe(false);
    } finally {
      if (appliedToMain) cleanupFileFromMain(file, suffix);
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
