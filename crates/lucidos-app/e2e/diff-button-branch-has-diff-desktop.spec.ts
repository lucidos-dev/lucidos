import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';
import {
  psql, git, WORKSPACE, cleanupCCThread, cleanupFileFromMain,
} from './db-helpers';
import { writeFileSync } from 'fs';
import { resolve } from 'path';
import { randomUUID } from 'crypto';
import type { Page } from './fixtures';

/**
 * Desktop-scoped (`-desktop` suffix → chromium only): this verifies the backend
 * projection→SSE→banner reactivity, which is layout-independent — one project is
 * enough. The mobile rendering of the same split button is covered by
 * `change-actions-split-mobile.spec.ts`.
 *
 * Verifies the WaitingBanner Diff affordance reacts to `coding_agent_has_diff`
 * flips driven by the projection. Diff SHOWS only when there is a diff — no diff
 * means no Diff affordance at all (not a greyed/disabled one). Diff is always a
 * standalone top-level button (never folded into the Apply/Discard split menu),
 * so `diffVisible()` just checks for that button.
 *
 *   - CC thread, no diff     → Diff hidden.
 *   - ChangeProposed lands   → projection flips coding_agent_has_diff=TRUE → SSE → Diff appears (standalone button).
 *   - ChangeApplied lands    → projection flips coding_agent_has_diff=FALSE → SSE → Diff disappears.
 *
 * The seed step uses `POST /api/v1/internal/seed-change-for-test`, which emits
 * `ChangeProposed` through the live EventBus, so the projection write AND
 * the per-event aggregate broadcast both run — same path the production CC
 * commit hook would take. Direct INSERT into `events` would skip both.
 *
 * Apply uses the real `POST /api/v1/changes/{id}/apply` endpoint, which fires
 * `ChangeApplied` through the EventBus on success.
 */

/** Whether the Diff affordance is currently available. Diff is always a
 *  standalone top-level button in the banner (never folded into the Apply/Discard
 *  split menu), so this is a single top-level check. Used inside `expect.poll` so
 *  it converges as SSE settles the projection. */
async function diffVisible(page: Page): Promise<boolean> {
  return page.locator('.thread-action-buttons:visible button:has-text("Diff")').first()
    .isVisible().catch(() => false);
}

/** Seed a CC thread in the 'waiting' state with NO changes (so initial
 *  coding_agent_has_diff is FALSE). Mirrors the helper in cc-stuck-waiting.spec.ts. */
function seedWaitingThread(suffix: string): { threadId: string } {
  const threadId = randomUUID();
  const now = new Date().toISOString();

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo, coding_agent_applying, coding_agent_has_diff) VALUES ('${threadId}', 'E2E Diff Branch ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'inbox', true, 0, false, false, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"test","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Done.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'CodingAgentIdled', '{"has_changes":false,"is_external_repo":false,"requires_restart":false}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));

  return { threadId };
}

/** Create a real git branch with a single commit so the apply path can later
 *  merge it back into main. Returns the branch name and file path so cleanup
 *  can drop both. */
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

/** Hit the dev-only seed endpoint which emits ChangeProposed (aggregate path)
 *  via the live EventBus. The projection sets coding_agent_has_diff=TRUE in the
 *  same tx and the per-event aggregate is broadcast over SSE. */
async function seedChangeProposed(page: Page, opts: {
  changeId: string;
  threadId: string;
  branch: string;
  file: string;
  description: string;
}): Promise<void> {
  const resp = await page.request.post('/api/v1/internal/seed-change-for-test', {
    data: {
      change_id: opts.changeId,
      thread_id: opts.threadId,
      branch_name: opts.branch,
      repo_root: WORKSPACE,
      description: opts.description,
      files: [opts.file],
      requires_restart: false,
      hardened: true,
    },
  });
  expect(resp.ok(), `seed-change-for-test failed: ${resp.status()} ${await resp.text()}`).toBeTruthy();
}

test.describe('WaitingBanner Diff button reacts to coding_agent_has_diff', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // Settle stale waiting CC threads so their banners don't fight the test
    // for focus when localStorage doesn't pin our thread fast enough.
    psql("UPDATE thread_summaries SET status = 'idle', archive_state = 'archived' WHERE status = 'waiting' AND is_coding_agent = true");
  });

  /** Anchor the wait on the banner's Archive button — present for a waiting CC
   *  thread with no diff — so we know the banner rendered before asserting the
   *  Diff button is absent. */
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
      await navigateToApp(page);

      // The banner renders (Archive is offered) but with no diff there is no
      // Diff affordance at all — not a disabled one.
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
      await navigateToApp(page);

      // No diff yet → no Diff affordance.
      await expect(archiveButton(page)).toBeVisible({ timeout: 15_000 });
      expect(await diffVisible(page)).toBe(false);

      await seedChangeProposed(page, {
        changeId, threadId, branch, file,
        description: `E2E diff enable ${suffix}`,
      });

      // SSE delivers the new aggregate → coding_agent_has_diff=TRUE → the Diff
      // affordance appears (a standalone top-level button, alongside the
      // Apply/Discard split button). Generous timeout to cover slow CI without
      // masking flakes.
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
      await navigateToApp(page);

      await expect(archiveButton(page)).toBeVisible({ timeout: 15_000 });

      await seedChangeProposed(page, {
        changeId, threadId, branch, file,
        description: `E2E diff disable ${suffix}`,
      });
      await expect.poll(() => diffVisible(page), { timeout: 10_000 }).toBe(true);

      // Apply via the real API → ChangeApplied → projection clears coding_agent_has_diff.
      const applyResp = await page.request.post(`/api/v1/changes/${changeId}/apply`);
      expect(applyResp.ok(), `apply failed: ${applyResp.status()} ${await applyResp.text()}`).toBeTruthy();
      appliedToMain = true;

      await expect.poll(() => diffVisible(page), { timeout: 10_000 }).toBe(false);
    } finally {
      if (appliedToMain) cleanupFileFromMain(file, suffix);
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
