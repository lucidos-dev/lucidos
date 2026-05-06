import { test, expect } from '@playwright/test';
import { randomUUID } from 'crypto';
import { assertHealthy, navigateToApp } from './helpers';
import { psql } from './db-helpers';

test.describe('Error display states', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('malformed chat stream request shows error response', async ({ page }) => {
    // Send a request with missing required fields
    const resp = await page.request.post('/api/chat/stream', {
      headers: { 'content-type': 'application/json' },
      data: JSON.stringify({ text: '' }),
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
    expect(resp.status()).toBeLessThan(500);
  });

  test('chat stream with invalid thread_id returns error', async ({ page }) => {
    const resp = await page.request.post('/api/chat/stream', {
      headers: { 'content-type': 'application/json' },
      data: JSON.stringify({ text: 'hello', thread_id: 'not-a-valid-uuid' }),
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
    expect(resp.status()).toBeLessThan(500);
  });

  test('non-existent API endpoint returns 404', async ({ page }) => {
    const resp = await page.request.get('/api/nonexistent', {
      failOnStatusCode: false,
    });
    expect(resp.status()).toBe(404);
  });

  test('changes API with invalid change ID returns error', async ({ page }) => {
    const resp = await page.request.post('/api/changes/not-a-uuid/apply', {
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
  });

  test('engine-restart-then-recovered turn does NOT show "Response interrupted"', async ({ page }) => {
    // Regression: when the engine restarts mid-turn, recovery emits
    // ResponseAborted, then the rerun emits ResponseGenerated for the SAME
    // request_event_id. The exchange's terminal verdict must be the LATER
    // success — not the earlier abort. Pre-fix: the badge falsely read
    // "Response interrupted" / showed the aborted warning marker.
    const threadId = randomUUID();
    const userMsgId = randomUUID();
    const abortedId = randomUUID();
    const textStreamedId = randomUUID();
    const generatedId = randomUUID();
    const t0 = new Date(Date.now() - 4_000).toISOString();
    const t1 = new Date(Date.now() - 3_000).toISOString();
    const t2 = new Date(Date.now() - 2_000).toISOString();
    const t3 = new Date(Date.now() - 1_000).toISOString();

    const userPayload = JSON.stringify({ text: 'recovery e2e test', channel: 'chat' });
    const abortedPayload = JSON.stringify({
      text: 'This response was interrupted by an engine restart.',
      images: [],
      request_event_id: userMsgId,
    });
    // exchangeResponseText() pulls visible text from TextStreamed events, not
    // from the terminal payload — seed a TextStreamed for the rerun output so
    // the response content actually renders in the DOM.
    const textStreamedPayload = JSON.stringify({
      text: 'final answer after rerun',
      request_event_id: userMsgId,
    });
    const generatedPayload = JSON.stringify({
      text: 'final answer after rerun',
      images: [],
      request_event_id: userMsgId,
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_cc, active_children_count, cc_has_changes, cc_requires_restart, cc_is_external_repo) VALUES ('${threadId}', 'Recovery rerun e2e', 'chat', '${t3}', 1, false, true, 'idle', 'archived', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '${userPayload}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${textStreamedId}', 'TextStreamed', '${textStreamedPayload}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${generatedId}', 'ResponseGenerated', '${generatedPayload}'::jsonb, '${t3}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      const exchange = page.locator('.chat-exchange:visible').first();
      await expect(exchange).toContainText('final answer after rerun', { timeout: 10_000 });

      // A later same-request_event_id ResponseGenerated must supersede the
      // earlier ResponseAborted: no abort marker, no aborted badge, no
      // "Response interrupted" copy inside the exchange.
      await expect(exchange.locator('.response-aborted-marker')).toHaveCount(0);
      await expect(exchange.locator('.exchange-status-aborted')).toHaveCount(0);
      await expect(exchange.getByText('Response interrupted')).toHaveCount(0);
      await expect(exchange.locator('.exchange-status-done')).toBeVisible({ timeout: 5_000 });
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});

// ---------------------------------------------------------------------------
// Resume-after-restart UI: AbortPanel + ResumePanel + Continue button.
// Each test seeds a thread directly via psql() (so we don't actually need to
// crash + restart the engine) and asserts the rendered DOM matches the
// design.
// ---------------------------------------------------------------------------
test.describe('Resume after restart — boundary panels', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('chat thread aborted by engine: System chip + Continue button', async ({ page }) => {
    const threadId = randomUUID();
    const userMsgId = randomUUID();
    const abortedId = randomUUID();
    const t0 = new Date(Date.now() - 3_000).toISOString();
    const t1 = new Date(Date.now() - 2_000).toISOString();

    const userPayload = JSON.stringify({ text: 'fix the bug', channel: 'chat' });
    const abortedPayload = JSON.stringify({
      text: '',
      images: [],
      request_event_id: userMsgId,
      actor: { kind: 'engine', reason: { kind: 'orphan_recovery' } },
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_cc, active_children_count, cc_has_changes, cc_requires_restart, cc_is_external_repo) VALUES ('${threadId}', 'Engine abort e2e', 'chat', '${t1}', 1, false, false, 'idle', 'inbox', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '${userPayload}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      // The abort boundary opens its own exchange whose initiator label is
      // the engine label (⚙ Lucidos Engine). The Continue button sits in the
      // initiator footer.
      const exchanges = page.locator('.chat-exchange:visible');
      await expect(exchanges).toHaveCount(2, { timeout: 10_000 });
      const abortExchange = exchanges.nth(1);
      await expect(abortExchange.locator('.initiator-label')).toContainText('Lucidos Engine');
      await expect(abortExchange.getByText('Response interrupted')).toBeVisible();
      await expect(abortExchange.getByRole('button', { name: 'Continue' })).toBeVisible();
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('chat thread aborted by /api/restart: You chip + Continue button', async ({ page }) => {
    const threadId = randomUUID();
    const userMsgId = randomUUID();
    const abortedId = randomUUID();
    const t0 = new Date(Date.now() - 3_000).toISOString();
    const t1 = new Date(Date.now() - 2_000).toISOString();

    const userPayload = JSON.stringify({ text: 'fix the bug', channel: 'chat' });
    const abortedPayload = JSON.stringify({
      text: '',
      images: [],
      request_event_id: userMsgId,
      actor: { kind: 'device', device_id: 'd-test', label: 'My Mac' },
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_cc, active_children_count, cc_has_changes, cc_requires_restart, cc_is_external_repo) VALUES ('${threadId}', 'Device abort e2e', 'chat', '${t1}', 1, false, false, 'idle', 'inbox', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '${userPayload}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      const exchanges = page.locator('.chat-exchange:visible');
      await expect(exchanges).toHaveCount(2, { timeout: 10_000 });
      const abortExchange = exchanges.nth(1);
      await expect(abortExchange.locator('.initiator-label')).toContainText('You');
      await expect(abortExchange.getByRole('button', { name: 'Continue' })).toBeVisible();
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('Continue button hides after a SessionRecovered lands', async ({ page }) => {
    // Same shape as the engine-abort test, but with a trailing SessionRecovered
    // — the AbortPanel must render WITHOUT the Continue button.
    const threadId = randomUUID();
    const userMsgId = randomUUID();
    const abortedId = randomUUID();
    const recoveredId = randomUUID();
    const t0 = new Date(Date.now() - 4_000).toISOString();
    const t1 = new Date(Date.now() - 3_000).toISOString();
    const t2 = new Date(Date.now() - 2_000).toISOString();

    const abortedPayload = JSON.stringify({
      text: '',
      images: [],
      request_event_id: userMsgId,
      actor: { kind: 'engine', reason: { kind: 'orphan_recovery' } },
    });
    const recoveredPayload = JSON.stringify({
      branch: '',
      actor: { kind: 'device', device_id: 'd-test', label: 'My Mac' },
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_cc, active_children_count, cc_has_changes, cc_requires_restart, cc_is_external_repo) VALUES ('${threadId}', 'Resumed e2e', 'chat', '${t2}', 1, false, false, 'idle', 'archived', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '{"text":"hi","channel":"chat"}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${recoveredId}', 'SessionRecovered', '${recoveredPayload}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      // Three exchanges: original, abort boundary, resume.
      const exchanges = page.locator('.chat-exchange:visible');
      await expect(exchanges).toHaveCount(3, { timeout: 10_000 });
      // The abort boundary's Continue button must NOT render once a
      // SessionRecovered exists later in the thread.
      const abortExchange = exchanges.nth(1);
      await expect(abortExchange.getByRole('button', { name: 'Continue' })).toHaveCount(0);
      // The resume exchange shows the device-attributed initiator.
      const resumeExchange = exchanges.nth(2);
      await expect(resumeExchange.locator('.initiator-label')).toContainText('You');
      await expect(resumeExchange.getByText('Resumed after engine restart')).toBeVisible();
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('SessionRecovered with engine note: subline counts tool entries', async ({ page }) => {
    const threadId = randomUUID();
    const userMsgId = randomUUID();
    const abortedId = randomUUID();
    const recoveredId = randomUUID();
    const injectedId = randomUUID();
    const t0 = new Date(Date.now() - 5_000).toISOString();
    const t1 = new Date(Date.now() - 4_000).toISOString();
    const t2 = new Date(Date.now() - 3_000).toISOString();
    const t3 = new Date(Date.now() - 2_000).toISOString();

    const abortedPayload = JSON.stringify({
      text: '',
      images: [],
      request_event_id: userMsgId,
      actor: { kind: 'engine', reason: { kind: 'orphan_recovery' } },
    });
    const recoveredPayload = JSON.stringify({
      branch: '',
      actor: { kind: 'device', device_id: 'd-test', label: 'My Mac' },
    });
    // Engine note text must include 2 bullet-style tool lines so the subline
    // reads "Reminded the model about 2 prior tool calls".
    const noteText =
      '[Engine note — this is a rerun]\n' +
      'Your previous attempt at this turn was interrupted by an engine restart.\n' +
      'The interrupted run performed the following actions before the abort:\n' +
      '- send_notification(Hi) → ok\n' +
      '- read_file(foo.txt) → contents\n' +
      'Decide whether to re-run them.';
    const injectedPayload = JSON.stringify({
      text: noteText,
      mode: 'engine',
      origin: { kind: 'engine', reason: { kind: 'session_recovered' } },
      request_event_id: recoveredId,
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_cc, active_children_count, cc_has_changes, cc_requires_restart, cc_is_external_repo) VALUES ('${threadId}', 'Engine note e2e', 'chat', '${t3}', 1, false, false, 'idle', 'archived', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '{"text":"hi","channel":"chat"}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${recoveredId}', 'SessionRecovered', '${recoveredPayload}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${injectedId}', 'UserPromptInjected', '${injectedPayload}'::jsonb, '${t3}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      const exchanges = page.locator('.chat-exchange:visible');
      await expect(exchanges).toHaveCount(3, { timeout: 10_000 });
      const resumeExchange = exchanges.nth(2);
      await expect(resumeExchange.getByText('Reminded the model about 2 prior tool calls')).toBeVisible();
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
