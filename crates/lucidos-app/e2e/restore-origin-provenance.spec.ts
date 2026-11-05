import { fileURLToPath } from 'node:url';
import { basename } from 'node:path';
import { test, expect } from './fixtures';
import {
  apiRequest, assertHealthy, navigateToApp, newThread, pickComposeDestination, sendMessage, waitForResponse,
} from './helpers';
import { clearAllThreads, psql } from './db-helpers';

const FAKE_CLAUDE = fileURLToPath(new URL('./fake-claude-code/claude.mjs', import.meta.url));
const PREF = 'coding_agent_claude_path';

// A bare engine must refuse this proof: the harness must attach a registered
// gateway identity. A private gateway cannot launch from a coding-agent worktree.
for (const selfRestore of [false, true]) {
  test(`an engine-spawned agent ${selfRestore ? 'refuses CLI self-restore' : 'proves its restore origin'} over its source socket`, async ({ page }) => {
    const workspace = process.env.E2E_WORKSPACE;
    expect(workspace, 'the disposable workspace exported by the e2e harness').toBeTruthy();
    clearAllThreads();
    await assertHealthy(page);
    const old = psql(`SELECT value FROM preferences WHERE key = '${PREF}'`);
    const res = await apiRequest(page).put(`/api/v1/preferences?key=${PREF}`, { data: { value: FAKE_CLAUDE } });
    expect(res.ok()).toBe(true);
    try {
      await navigateToApp(page);
      await newThread(page);
      await pickComposeDestination(page);
      await sendMessage(page, selfRestore ? 'Probe snapshot self-restore refusal.' : 'Probe snapshot restore origin.');
      const response = await waitForResponse(page);
      const thread = psql('SELECT thread_id FROM thread_summaries WHERE NOT is_home ORDER BY created_at DESC LIMIT 1');
      await expect(response).toContainText('"verified":true');
      const proof = JSON.parse((await response.textContent())!.trim());
      expect(proof).toEqual({ verified: true, workspace_id: basename(workspace!), source_thread_id: thread,
        ...(selfRestore ? { self_restore_refused: true } : {}),
      });
    } finally {
      if (old) {
        const restored = await apiRequest(page).put(`/api/v1/preferences?key=${PREF}`, { data: { value: old } });
        expect(restored.ok()).toBe(true);
      } else {
        psql(`DELETE FROM preferences WHERE key = '${PREF}'`);
      }
    }
  });
}
