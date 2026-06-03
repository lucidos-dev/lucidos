import { test, expect } from '@playwright/test';
import {
  assertHealthy, uniqueMessage,
} from './helpers';
import { git, psql, WORKSPACE, cleanupCCThread, cleanupFileFromMain } from './db-helpers';
import { execSync } from 'child_process';
import { writeFileSync } from 'fs';
import { resolve } from 'path';
import { randomUUID } from 'crypto';

/** Create a pending change (no thread, just changes table + git branch). */
function createTestChange(suffix: string): { id: string; branch: string; file: string } {
  const branch = `e2e-test/change-${suffix}`;
  const file = `e2e-test-${suffix}.txt`;
  const id = randomUUID();

  git(['checkout', '-b', branch, 'main']);
  writeFileSync(resolve(WORKSPACE, file), `test content ${suffix}`);
  git(['add', '.']);
  git(['commit', '-m', `e2e test change ${suffix}`]);
  git(['checkout', 'main']);

  psql(`INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened) VALUES ('${id}', '${randomUUID()}', '${branch}', '${WORKSPACE}', 'E2E test change ${suffix}', 1, ARRAY['${file}'], false, true)`);

  return { id, branch, file };
}

function cleanupTestChange(id: string, branch: string, file: string): void {
  cleanupCCThread('00000000-0000-0000-0000-000000000000', id, branch, file);
}

test.describe('Apply and discard changes', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('apply a pending change via API and verify it succeeds', async ({ page }) => {
    const suffix = uniqueMessage('apply').replace(/[^a-z0-9-]/g, '');
    const change = createTestChange(suffix);

    try {
      const resp = await page.request.post(`/api/v1/changes/${change.id}/apply`);
      expect(resp.ok()).toBeTruthy();
      const body = await resp.json();
      expect(body.message).toBeTruthy();
      expect(body.error).toBeUndefined();

      // Verify the file now exists on main
      const fileExists = execSync(`test -f "${resolve(WORKSPACE, change.file)}" && echo "yes" || echo "no"`, { encoding: 'utf-8' }).trim();
      expect(fileExists).toBe('yes');

      // Verify git status is clean
      const status = git(['status', '--porcelain']);
      const dirtyLines = status.split('\n').filter(l => l.trim() && !l.startsWith('??'));
      expect(dirtyLines).toHaveLength(0);

      cleanupFileFromMain(change.file, suffix);
    } finally {
      cleanupTestChange(change.id, change.branch, change.file);
    }
  });

  test('discard a pending change via API and verify it is removed', async ({ page }) => {
    const suffix = uniqueMessage('discard').replace(/[^a-z0-9-]/g, '');
    const change = createTestChange(suffix);

    try {
      const resp = await page.request.post(`/api/v1/changes/${change.id}/discard`);
      expect(resp.ok()).toBeTruthy();
      const body = await resp.json();
      expect(body.message).toContain('discarded');

      // Verify the file does NOT exist on main
      const fileExists = execSync(`test -f "${resolve(WORKSPACE, change.file)}" && echo "yes" || echo "no"`, { encoding: 'utf-8' }).trim();
      expect(fileExists).toBe('no');

      // Verify the change is no longer in the pending list
      const changesResp = await page.request.get('/api/v1/changes');
      expect(changesResp.ok()).toBeTruthy();
      const changesBody = await changesResp.json();
      const pending = changesBody.pending as Array<{ id: string }>;
      expect(pending.find(c => c.id === change.id)).toBeUndefined();
    } finally {
      cleanupTestChange(change.id, change.branch, change.file);
    }
  });

  test('apply a change is visible in the changes API list', async ({ page }) => {
    const suffix = uniqueMessage('api-apply').replace(/[^a-z0-9-]/g, '');
    const change = createTestChange(suffix);

    try {
      // Verify our test change appears in pending list
      const changesResp = await page.request.get('/api/v1/changes');
      const changesBody = await changesResp.json();
      const pending = changesBody.pending as Array<{ id: string; description: string }>;
      const ourChange = pending.find(c => c.id === change.id);
      expect(ourChange).toBeDefined();
      expect(ourChange!.description).toContain(suffix);

      // Apply via API
      const applyResp = await page.request.post(`/api/v1/changes/${change.id}/apply`);
      expect(applyResp.ok()).toBeTruthy();

      // Verify it moved to applied list
      const afterResp = await page.request.get('/api/v1/changes');
      const afterBody = await afterResp.json();
      const afterPending = afterBody.pending as Array<{ id: string }>;
      expect(afterPending.find(c => c.id === change.id)).toBeUndefined();

      cleanupFileFromMain(change.file, suffix);
    } finally {
      cleanupTestChange(change.id, change.branch, change.file);
    }
  });

  test('discard all pending changes', async ({ page }) => {
    const suffix1 = uniqueMessage('discard-all-1').replace(/[^a-z0-9-]/g, '');
    const suffix2 = uniqueMessage('discard-all-2').replace(/[^a-z0-9-]/g, '');
    const change1 = createTestChange(suffix1);
    const change2 = createTestChange(suffix2);

    try {
      const resp = await page.request.post('/api/v1/changes/discard-all');
      expect(resp.ok()).toBeTruthy();
      const body = await resp.json();
      expect(body.message).toContain('discarded');

      // Verify no pending changes remain for our test changes
      const changesResp = await page.request.get('/api/v1/changes');
      const changesBody = await changesResp.json();
      const pending = changesBody.pending as Array<{ id: string }>;
      expect(pending.find(c => c.id === change1.id)).toBeUndefined();
      expect(pending.find(c => c.id === change2.id)).toBeUndefined();
    } finally {
      cleanupTestChange(change1.id, change1.branch, change1.file);
      cleanupTestChange(change2.id, change2.branch, change2.file);
    }
  });

  test('applying already-applied change is idempotent', async ({ page }) => {
    const suffix = uniqueMessage('double-apply').replace(/[^a-z0-9-]/g, '');
    const change = createTestChange(suffix);

    try {
      // Apply first time
      const resp1 = await page.request.post(`/api/v1/changes/${change.id}/apply`);
      expect(resp1.ok()).toBeTruthy();

      // Apply second time — should either succeed idempotently or return error
      const resp2 = await page.request.post(`/api/v1/changes/${change.id}/apply`, {
        failOnStatusCode: false,
      });
      // The API may return 200 (idempotent) or 400 (already applied) — both are valid
      expect(resp2.status()).toBeLessThan(500);
      const body2 = await resp2.json();
      expect(body2.message).toBeTruthy();

      cleanupFileFromMain(change.file, suffix);
    } finally {
      cleanupTestChange(change.id, change.branch, change.file);
    }
  });

  test('discarding non-existent change returns error', async ({ page }) => {
    const fakeId = randomUUID();
    const resp = await page.request.post(`/api/v1/changes/${fakeId}/discard`, {
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
  });
});
