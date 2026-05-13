import { test, expect, request as pwRequest } from '@playwright/test';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, getBaseUrl } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * Browser e2e for the CC permission prompt UI.
 *
 * The "MCP server inside lucidos-cli" half is exercised by Rust unit tests
 * (lucidos-cli) and the API e2e test (permission_prompt_test.rs). This spec
 * covers the browser side end-to-end:
 *   - Seed a CC thread, then POST /api/internal/permission-prompt from the
 *     test (simulating lucidos-cli's MCP subprocess).
 *   - Wait for the engine to emit CodingAgentPermissionRequest. The engine
 *     promotes that event into its own divider exchange (initiator panel,
 *     "You" chip), with the PermissionBody as the body — not inline in the
 *     prior CC response panel.
 *   - Click Allow → engine resolves the oneshot via /api/mcp/consent (driven
 *     by the click), responds to the still-blocked permission-prompt POST
 *     with `{ allowed: true }`, and emits CodingAgentPermissionResolved.
 *   - The SAME initiator panel flips in place to its answered state.
 */
test.describe('CC permission prompt — Allow / Deny flow', () => {
  test('Allow path resolves the request and flips the card to Allowed', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const now = new Date().toISOString();

    const msgEventId = randomUUID();
    const sessionStartedId = randomUUID();
    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_cc, active_children_count) VALUES ('${threadId}', 'CC Permission E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'running', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"edit my skill","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${sessionStartedId}', 'SessionStarted', '{"session_id":"sess-perm-e2e-${suffix}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const apiContext = await pwRequest.newContext({
      baseURL: getBaseUrl(),
      ignoreHTTPSErrors: true,
    });
    let promptResponse: Promise<Awaited<ReturnType<typeof apiContext.post>>> | undefined;
    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Permission E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      // Fire the (blocking) permission-prompt request from the test, mimicking
      // what lucidos-cli's MCP subprocess would do. Don't await yet — the
      // handler blocks on the user's decision.
      promptResponse = apiContext.post('/api/internal/permission-prompt', {
        data: {
          thread_id: threadId,
          tool_use_id: `tu-perm-e2e-${suffix}`,
          tool_name: 'Edit',
          input: { file_path: `/tmp/cc-perm-e2e-${suffix}.txt` },
        },
      });

      // The PermissionBody renders as the body of a divider initiator panel.
      // Both SplitLayout (desktop) and MobileSwipeContainer (mobile) render
      // simultaneously; scope to the visible copy.
      const panel = page
        .locator('.initiator-panel-user:visible:has(.cc-permission-body)')
        .first();
      await expect(panel).toBeVisible({ timeout: 15_000 });
      await expect(panel.locator('.initiator-label')).toHaveText('You');
      const body = panel.locator('.initiator-body .cc-permission-body').first();
      await expect(body).toContainText(/Edit/);
      await expect(body).toContainText(`/tmp/cc-perm-e2e-${suffix}.txt`);

      await body.locator('button', { hasText: /^Allow once$/ }).click();

      // The blocked POST should now return with allowed=true.
      const resolved = await promptResponse;
      promptResponse = undefined;
      expect(resolved.status()).toBe(200);
      const respBody = await resolved.json();
      expect(respBody.allowed).toBe(true);

      // The SAME panel flips in place — the answered body lives inside the
      // same initiator panel, no new divider materialized for the click. The
      // picked button keeps its semantic styling with a check; the rejected
      // ones are disabled and struck through.
      const answered = panel.locator('.initiator-body .cc-permission-body-answered').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      const picked = answered.locator('button.cc-permission-btn-picked');
      await expect(picked).toHaveText(/Allow once/);
      await expect(picked).toBeDisabled();
      await expect(answered.locator('button.cc-permission-btn-rejected', { hasText: /^Deny$/ })).toBeDisabled();

      // Both events landed in the DB and are persisted.
      await expect.poll(
        () => psql(`SELECT COUNT(*) FROM events WHERE aggregate_id = '${threadId}' AND event_type = 'CodingAgentPermissionResolved'`),
        { intervals: [400], timeout: 10_000 },
      ).toBe('1');
    } finally {
      // If the test failed before clicking Allow, drain the pending request so
      // it doesn't dangle and pin the next test's pending_mcp_consent slot.
      if (promptResponse) {
        await promptResponse.catch(() => undefined);
      }
      await apiContext.dispose();
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('Deny path resolves with allowed=false and surfaces the denial', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const now = new Date().toISOString();

    const msgEventId = randomUUID();
    const sessionStartedId = randomUUID();
    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_cc, active_children_count) VALUES ('${threadId}', 'CC Permission Deny E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'running', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"do something risky","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${sessionStartedId}', 'SessionStarted', '{"session_id":"sess-deny-e2e-${suffix}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const apiContext = await pwRequest.newContext({
      baseURL: getBaseUrl(),
      ignoreHTTPSErrors: true,
    });
    let promptResponse: Promise<Awaited<ReturnType<typeof apiContext.post>>> | undefined;
    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Permission Deny E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      promptResponse = apiContext.post('/api/internal/permission-prompt', {
        data: {
          thread_id: threadId,
          tool_use_id: `tu-deny-e2e-${suffix}`,
          tool_name: 'Bash',
          input: { command: 'rm -rf /tmp/lucidos-deny-test' },
        },
      });

      const panel = page
        .locator('.initiator-panel-user:visible:has(.cc-permission-body)')
        .first();
      await expect(panel).toBeVisible({ timeout: 15_000 });
      const body = panel.locator('.initiator-body .cc-permission-body').first();
      await expect(body).toContainText(/Bash/);

      await body.locator('button', { hasText: /^Deny$/ }).click();

      const resolved = await promptResponse;
      promptResponse = undefined;
      expect(resolved.status()).toBe(200);
      const respBody = await resolved.json();
      expect(respBody.allowed).toBe(false);

      const answered = panel.locator('.initiator-body .cc-permission-body-answered').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      const picked = answered.locator('button.cc-permission-btn-picked');
      await expect(picked).toHaveText(/Deny/);
      await expect(picked).toBeDisabled();
      await expect(answered.locator('button.cc-permission-btn-rejected', { hasText: /^Allow once$/ })).toBeDisabled();
    } finally {
      if (promptResponse) {
        await promptResponse.catch(() => undefined);
      }
      await apiContext.dispose();
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('Always allow Skill(plugin:*) appends pattern to cc-allowed-tools', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const now = new Date().toISOString();
    // Unique plugin name keeps parallel/repeat runs isolated and identifies test debris.
    const plugin = `e2e-${suffix}`;
    const pattern = `Skill(${plugin}:*)`;

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_cc, active_children_count) VALUES ('${threadId}', 'CC Always Allow E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'running', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"run a skill","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-aa-e2e-${suffix}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const apiContext = await pwRequest.newContext({
      baseURL: getBaseUrl(),
      ignoreHTTPSErrors: true,
    });
    let promptResponse: Promise<Awaited<ReturnType<typeof apiContext.post>>> | undefined;

    // Snapshot the file so we can restore it (don't pollute the real ~/.lucidos).
    const snapshotResp = await apiContext.get('/api/cc-allowed-tools');
    expect(snapshotResp.status()).toBe(200);
    const snapshot = (await snapshotResp.json()).contents as string;

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Always Allow E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      promptResponse = apiContext.post('/api/internal/permission-prompt', {
        data: {
          thread_id: threadId,
          tool_use_id: `tu-aa-e2e-${suffix}`,
          tool_name: 'Skill',
          input: { skill: `${plugin}:demo` },
        },
      });

      const body = page
        .locator('.initiator-panel-user:visible:has(.cc-permission-body) .initiator-body .cc-permission-body')
        .first();
      await expect(body).toBeVisible({ timeout: 15_000 });

      // Click the narrow "Always allow Skill(plugin:*)" button — second-row button.
      await body.locator('button', { hasText: new RegExp(`Always allow.*${plugin}`) }).click();

      const resolved = await promptResponse;
      promptResponse = undefined;
      expect(resolved.status()).toBe(200);
      expect((await resolved.json()).allowed).toBe(true);

      // File now contains the pattern.
      const after = await apiContext.get('/api/cc-allowed-tools');
      expect(after.status()).toBe(200);
      const contents = (await after.json()).contents as string;
      expect(contents).toContain(pattern);
    } finally {
      if (promptResponse) {
        await promptResponse.catch(() => undefined);
      }
      // Restore the file unconditionally so the user's real allowlist is untouched.
      await apiContext.put('/api/cc-allowed-tools', { data: { contents: snapshot } }).catch(() => undefined);
      await apiContext.dispose();
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('Tool permissions settings page round-trips edits via PUT/GET', async ({ page }) => {
    await assertHealthy(page);

    const apiContext = await pwRequest.newContext({
      baseURL: getBaseUrl(),
      ignoreHTTPSErrors: true,
    });

    const snapshotResp = await apiContext.get('/api/cc-allowed-tools');
    expect(snapshotResp.status()).toBe(200);
    const snapshot = (await snapshotResp.json()).contents as string;

    const sentinel = `# e2e-${randomUUID().slice(0, 8)}\nReadOnly\n`;

    try {
      // Write via PUT, read back via GET — the section component uses the same
      // endpoints, so this proves the wire contract used by the UI.
      const put = await apiContext.put('/api/cc-allowed-tools', { data: { contents: sentinel } });
      expect(put.status()).toBe(204);
      const reread = await apiContext.get('/api/cc-allowed-tools');
      expect(reread.status()).toBe(200);
      expect((await reread.json()).contents).toBe(sentinel);
    } finally {
      await apiContext.put('/api/cc-allowed-tools', { data: { contents: snapshot } }).catch(() => undefined);
      await apiContext.dispose();
    }
  });
});
