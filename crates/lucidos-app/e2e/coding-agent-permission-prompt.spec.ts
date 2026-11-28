import { test, expect, request as pwRequest } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, getBaseUrl } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';
import type { APIRequestContext, Locator, Page } from '@playwright/test';

async function openRemember(page: Page, body: Locator): Promise<Locator> {
  await body.locator('button.permission-remember').click();
  const items = page.getByRole('menuitem');
  await expect(items.first()).toBeVisible();
  return items;
}

function postPermissionPrompt(
  apiContext: APIRequestContext,
  data: { thread_id: string; tool_use_id: string; tool_name: string; input: Record<string, unknown> },
) {
  return apiContext.post('/api/v1/internal/permission-prompt', { data, timeout: 0 });
}

test.describe('CC permission prompt — Allow / Deny flow', () => {
  test('Allow path resolves the request and flips the card to Allowed', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const now = new Date().toISOString();

    const msgEventId = randomUUID();
    const sessionStartedId = randomUUID();
    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Permission E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'running', 'inbox', true, 0)`,
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

      promptResponse = postPermissionPrompt(apiContext, {
        thread_id: threadId,
        tool_use_id: `tu-perm-e2e-${suffix}`,
        tool_name: 'Edit',
        input: { file_path: `/tmp/cc-perm-e2e-${suffix}.txt` },
      });

      const panel = page
        .locator('.initiator-panel-lucidos:visible:has(.permission-body)')
        .first();
      await expect(panel).toBeVisible({ timeout: 15_000 });
      await expect(panel.locator('.initiator-label')).toHaveText('Claude');
      const body = panel.locator('.initiator-body .permission-body').first();
      await expect(body).toContainText(`Let the coding agent edit /tmp/cc-perm-e2e-${suffix}.txt?`);

      await body.locator('button', { hasText: /^Allow once$/ }).click();

      const resolved = await promptResponse;
      promptResponse = undefined;
      expect(resolved.status()).toBe(200);
      const respBody = await resolved.json();
      expect(respBody.allowed).toBe(true);

      const answered = panel.locator('.initiator-body .permission-body-answered').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      const picked = answered.locator('button.permission-btn-picked');
      await expect(picked).toHaveText(/Allow once/);
      await expect(picked).toBeDisabled();
      await expect(answered.locator('button.permission-btn-rejected', { hasText: /^Deny$/ })).toBeDisabled();

      await expect.poll(
        () => psql(`SELECT COUNT(*) FROM events WHERE aggregate_id = '${threadId}' AND event_type = 'CodingAgentPermissionResolved'`),
        { intervals: [400], timeout: 10_000 },
      ).toBe('1');
    } finally {
      const parked = promptResponse?.catch(() => undefined);
      await apiContext.dispose();
      await parked;
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
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Permission Deny E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'running', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"do something risky","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${sessionStartedId}', 'SessionStarted', '{"session_id":"sess-deny-e2e-${suffix}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const apiContext = await pwRequest.newContext({
      baseURL: getBaseUrl(),
      ignoreHTTPSErrors: true,
    });
    let promptResponse: Promise<Awaited<ReturnType<typeof apiContext.post>>> | undefined;

    const snapshotResp = await apiContext.get('/api/v1/cc-allowed-tools');
    expect(snapshotResp.status()).toBe(200);
    const snapshot = (await snapshotResp.json()).contents as string;
    await apiContext.put('/api/v1/cc-allowed-tools', { data: { contents: '' } });

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Permission Deny E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      promptResponse = postPermissionPrompt(apiContext, {
        thread_id: threadId,
        tool_use_id: `tu-deny-e2e-${suffix}`,
        tool_name: 'Bash',
        input: { command: 'rm -rf /tmp/lucidos-deny-test' },
      });

      const panel = page
        .locator('.initiator-panel-lucidos:visible:has(.permission-body)')
        .first();
      await expect(panel).toBeVisible({ timeout: 15_000 });
      const body = panel.locator('.initiator-body .permission-body').first();
      await expect(body.locator('.permission-text pre')).toHaveText('rm -rf /tmp/lucidos-deny-test');

      const offer = JSON.parse(psql(
        `SELECT COALESCE(payload->'grant_offer', '{}'::jsonb) FROM events WHERE aggregate_id = '${threadId}' AND event_type = 'CodingAgentPermissionRequest'`,
      )) as { broad?: string; narrow?: string; session?: string };
      expect(offer.broad).toBe('Bash');
      await expect(body.locator('button', { hasText: /^Always allow/ })).toHaveCount(0);
      const items = await openRemember(page, body);
      await expect(page.getByRole('menuitem', { name: 'Always allow Bash', exact: true })).toHaveCount(1);
      await expect(page.getByRole('menuitem', { name: /^Allow .* for the rest of this thread$/ }))
        .toHaveCount(offer.session ? 1 : 0);
      await expect(page.getByRole('menuitem', { name: /^Always allow .+ commands$/ }))
        .toHaveCount(offer.narrow ? 1 : 0);
      await page.keyboard.press('Escape');
      await expect(items.first()).toBeHidden();

      await body.locator('button', { hasText: /^Deny$/ }).click();

      const resolved = await promptResponse;
      promptResponse = undefined;
      expect(resolved.status()).toBe(200);
      const respBody = await resolved.json();
      expect(respBody.allowed).toBe(false);

      const answered = panel.locator('.initiator-body .permission-body-answered').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      const picked = answered.locator('button.permission-btn-picked');
      await expect(picked).toHaveText(/Deny/);
      await expect(picked).toBeDisabled();
      await expect(answered.locator('button.permission-btn-rejected', { hasText: /^Allow once$/ })).toBeDisabled();
    } finally {
      const parked = promptResponse?.catch(() => undefined);
      await apiContext.put('/api/v1/cc-allowed-tools', { data: { contents: snapshot } }).catch(() => undefined);
      await apiContext.dispose();
      await parked;
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('Allow once takes visible keyboard focus, arrows step, Enter resolves', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const now = new Date().toISOString();

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Permission Keyboard E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'running', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"edit my file","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-perm-kbd-e2e-${suffix}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const apiContext = await pwRequest.newContext({
      baseURL: getBaseUrl(),
      ignoreHTTPSErrors: true,
    });
    let promptResponse: Promise<Awaited<ReturnType<typeof apiContext.post>>> | undefined;
    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Permission Keyboard E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      promptResponse = postPermissionPrompt(apiContext, {
        thread_id: threadId,
        tool_use_id: `tu-perm-kbd-e2e-${suffix}`,
        tool_name: 'Edit',
        input: { file_path: `/tmp/cc-perm-kbd-e2e-${suffix}.txt` },
      });

      const body = page.locator('.permission-body[data-role="card-choices"]:visible').first();
      await expect(body).toBeVisible({ timeout: 15_000 });

      const allowOnce = body.locator('button', { hasText: /^Allow once$/ });
      const deny = body.locator('button', { hasText: /^Deny$/ });
      const remember = body
        .locator('button.permission-remember, button:has-text("Always"), button:has-text("Allow for this thread")')
        .last();
      await expect(allowOnce).toBeFocused({ timeout: 10_000 });

      const ring = await page.evaluate(() => {
        const el = document.activeElement as HTMLElement | null;
        return el ? getComputedStyle(el).boxShadow : '';
      });
      expect(ring).not.toBe('');
      expect(ring).not.toBe('none');

      await page.keyboard.press('ArrowLeft');
      await expect(deny).toBeFocused();
      await page.keyboard.press('ArrowUp');
      await expect(remember).toBeFocused();
      await page.keyboard.press('ArrowDown');
      await expect(deny).toBeFocused();
      await page.keyboard.press('ArrowRight');
      await expect(allowOnce).toBeFocused();

      await page.keyboard.press('Enter');

      const resolved = await promptResponse;
      promptResponse = undefined;
      expect(resolved.status()).toBe(200);
      expect((await resolved.json()).allowed).toBe(true);

      const picked = page.locator('.permission-body-answered:visible button.permission-btn-picked').first();
      await expect(picked).toHaveText(/Allow once/, { timeout: 10_000 });
    } finally {
      const parked = promptResponse?.catch(() => undefined);
      await apiContext.dispose();
      await parked;
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
    const plugin = `e2e-${suffix}`;
    const pattern = `Skill(${plugin}:*)`;

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Always Allow E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'running', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"run a skill","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-aa-e2e-${suffix}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const apiContext = await pwRequest.newContext({
      baseURL: getBaseUrl(),
      ignoreHTTPSErrors: true,
    });
    let promptResponse: Promise<Awaited<ReturnType<typeof apiContext.post>>> | undefined;

    const snapshotResp = await apiContext.get('/api/v1/cc-allowed-tools');
    expect(snapshotResp.status()).toBe(200);
    const snapshot = (await snapshotResp.json()).contents as string;
    await apiContext.put('/api/v1/cc-allowed-tools', { data: { contents: '' } });

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Always Allow E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      promptResponse = postPermissionPrompt(apiContext, {
        thread_id: threadId,
        tool_use_id: `tu-aa-e2e-${suffix}`,
        tool_name: 'Skill',
        input: { skill: `${plugin}:demo` },
      });

      const body = page
        .locator('.initiator-panel-lucidos:visible:has(.permission-body) .initiator-body .permission-body')
        .first();
      await expect(body).toBeVisible({ timeout: 15_000 });

      await openRemember(page, body);
      await page.getByRole('menuitem', { name: new RegExp(`Always allow.*${plugin}`) }).click();

      const resolved = await promptResponse;
      promptResponse = undefined;
      expect(resolved.status()).toBe(200);
      expect((await resolved.json()).allowed).toBe(true);

      const after = await apiContext.get('/api/v1/cc-allowed-tools');
      expect(after.status()).toBe(200);
      const contents = (await after.json()).contents as string;
      expect(contents).toContain(pattern);
    } finally {
      const parked = promptResponse?.catch(() => undefined);
      await apiContext.put('/api/v1/cc-allowed-tools', { data: { contents: snapshot } }).catch(() => undefined);
      await apiContext.dispose();
      await parked;
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

    const snapshotResp = await apiContext.get('/api/v1/cc-allowed-tools');
    expect(snapshotResp.status()).toBe(200);
    const snapshot = (await snapshotResp.json()).contents as string;

    const sentinel = `# e2e-${randomUUID().slice(0, 8)}\nReadOnly\n`;

    try {
      const put = await apiContext.put('/api/v1/cc-allowed-tools', { data: { contents: sentinel } });
      expect(put.status()).toBe(204);
      const reread = await apiContext.get('/api/v1/cc-allowed-tools');
      expect(reread.status()).toBe(200);
      const grantLines = ((await reread.json()).contents as string)
        .split('\n')
        .map((line) => line.trim())
        .filter((line) => line && !line.startsWith('#'));
      expect(grantLines).toEqual(['ReadOnly']);
    } finally {
      await apiContext.put('/api/v1/cc-allowed-tools', { data: { contents: snapshot } }).catch(() => undefined);
      await apiContext.dispose();
    }
  });
});
