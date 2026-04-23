import { test, expect, request as pwRequest } from '@playwright/test';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, getBaseUrl } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * Browser e2e for the CC permission prompt UI.
 *
 * The "MCP server inside cognos-cli" half is exercised by Rust unit tests
 * (cognos-cli) and the API e2e test (permission_prompt_test.rs). This spec
 * covers the browser side end-to-end:
 *   - Seed a CC thread, then POST /api/internal/permission-prompt from the
 *     test (simulating cognos-cli's MCP subprocess).
 *   - Wait for the engine to emit CodingAgentPermissionRequest, render
 *     PermissionCard with Allow/Deny buttons.
 *   - Click Allow → engine resolves the oneshot via /api/mcp/consent (driven
 *     by the click), responds to the still-blocked permission-prompt POST
 *     with `{ allowed: true }`, and emits CodingAgentPermissionResolved.
 *   - Card flips to its answered state.
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
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_pinned, has_response, status, section, is_cc, active_children_count) VALUES ('${threadId}', 'CC Permission E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'running', 'unread', true, 0)`,
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
      // what cognos-cli's MCP subprocess would do. Don't await yet — the
      // handler blocks on the user's decision.
      promptResponse = apiContext.post('/api/internal/permission-prompt', {
        data: {
          thread_id: threadId,
          tool_use_id: `tu-perm-e2e-${suffix}`,
          tool_name: 'Edit',
          input: { file_path: `/tmp/cc-perm-e2e-${suffix}.txt` },
        },
      });

      // PermissionCard renders inline. Both SplitLayout (desktop) and
      // MobileSwipeContainer (mobile) render simultaneously; scope to visible.
      const card = page.locator(`.cc-permission-card:visible`).first();
      await expect(card).toBeVisible({ timeout: 15_000 });
      await expect(card).toContainText(/Edit/);
      await expect(card).toContainText(`/tmp/cc-perm-e2e-${suffix}.txt`);

      await card.locator('button', { hasText: /^Allow$/ }).click();

      // The blocked POST should now return with allowed=true.
      const resolved = await promptResponse;
      promptResponse = undefined;
      expect(resolved.status()).toBe(200);
      const body = await resolved.json();
      expect(body.allowed).toBe(true);

      // Card flips to its answered state.
      const answered = page.locator('.cc-permission-card-answered:visible').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      await expect(answered).toContainText(/Allowed/);

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
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_pinned, has_response, status, section, is_cc, active_children_count) VALUES ('${threadId}', 'CC Permission Deny E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'running', 'unread', true, 0)`,
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
          input: { command: 'rm -rf /tmp/cognos-deny-test' },
        },
      });

      const card = page.locator(`.cc-permission-card:visible`).first();
      await expect(card).toBeVisible({ timeout: 15_000 });
      await expect(card).toContainText(/Bash/);

      await card.locator('button', { hasText: /^Deny$/ }).click();

      const resolved = await promptResponse;
      promptResponse = undefined;
      expect(resolved.status()).toBe(200);
      const body = await resolved.json();
      expect(body.allowed).toBe(false);

      const answered = page.locator('.cc-permission-card-answered:visible').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      await expect(answered).toContainText(/Denied/);
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
});
