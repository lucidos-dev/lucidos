import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, navigateToApp } from './helpers';
import { psql } from './db-helpers';

test.describe('Error display states', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('malformed chat stream request shows error response', async ({ page }) => {
    const resp = await apiRequest(page).post('/api/v1/chat/stream', {
      headers: { 'content-type': 'application/json' },
      data: JSON.stringify({ text: '' }),
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
    expect(resp.status()).toBeLessThan(500);
  });

  test('chat stream with invalid thread_id returns error', async ({ page }) => {
    const resp = await apiRequest(page).post('/api/v1/chat/stream', {
      headers: { 'content-type': 'application/json' },
      data: JSON.stringify({ text: 'hello', thread_id: 'not-a-valid-uuid' }),
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
    expect(resp.status()).toBeLessThan(500);
  });

  test('non-existent API endpoint returns 404', async ({ page }) => {
    const resp = await page.request.get('/api/v1/nonexistent', {
      failOnStatusCode: false,
    });
    expect(resp.status()).toBe(404);
  });

  test('changes API with invalid change ID returns error', async ({ page }) => {
    const resp = await apiRequest(page).post('/api/v1/changes/not-a-uuid/apply', {
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
  });

  test('engine-restart-then-recovered turn does NOT show "Response interrupted"', async ({ page }) => {
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
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Recovery rerun e2e', 'chat', '${t3}', 1, false, true, 'idle', 'archived', false, 0, 'none', false)`,
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
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Engine abort e2e', 'chat', '${t1}', 1, false, false, 'idle', 'inbox', false, 0, 'none', false)`,
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
      await expect(abortExchange.locator('.initiator-actor')).toHaveCount(0);
      const card = abortExchange.locator('.event-row[data-kind="boundary"]');
      await expect(card.locator('.event-row-subject')).toHaveText('Response interrupted');
      await expect(card.locator('.event-row-actions').getByRole('button', { name: 'Continue' })).toBeVisible();
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('chat thread aborted by /api/v1/restart: iconless action label, no Continue button', async ({ page }) => {
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
      cause: 'engine_shutdown',
      actor: { kind: 'device', device_id: 'd-test' },
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Device abort e2e', 'chat', '${t1}', 1, false, false, 'idle', 'inbox', false, 0, 'none', false)`,
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
      await expect(abortExchange.locator('.initiator-actor')).toHaveCount(0);
      await expect(abortExchange.locator('.event-row[data-kind="boundary"] .event-row-state')).toHaveText('Paused by restart');
      await expect(abortExchange.getByRole('button', { name: 'Continue' })).toHaveCount(0);
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('Continue button hides after a ContinuationStarted lands', async ({ page }) => {
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
      actor: { kind: 'device', device_id: 'd-test' },
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Resumed e2e', 'chat', '${t2}', 1, false, false, 'idle', 'archived', false, 0, 'none', false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '{"text":"hi","channel":"chat"}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${recoveredId}', 'ContinuationStarted', '${recoveredPayload}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      const exchanges = page.locator('.chat-exchange:visible');
      await expect(exchanges).toHaveCount(3, { timeout: 10_000 });
      const abortExchange = exchanges.nth(1);
      await expect(abortExchange.getByRole('button', { name: 'Continue' })).toHaveCount(0);
      const resumeExchange = exchanges.nth(2);
      await expect(resumeExchange.locator('.initiator-panel')).toHaveCount(0);
      const resumeState = resumeExchange.locator('.event-row[data-kind="resume"] .event-row-state');
      await expect(resumeState).toHaveText('You continued');
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('ContinuationStarted with engine note: subline counts tool entries', async ({ page }) => {
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
      actor: { kind: 'device', device_id: 'd-test' },
    });
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
      origin: { kind: 'engine', reason: { kind: 'continuation_started' } },
      request_event_id: recoveredId,
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Engine note e2e', 'chat', '${t3}', 1, false, false, 'idle', 'archived', false, 0, 'none', false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '{"text":"hi","channel":"chat"}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${recoveredId}', 'ContinuationStarted', '${recoveredPayload}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${injectedId}', 'PromptInjected', '${injectedPayload}'::jsonb, '${t3}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      const exchanges = page.locator('.chat-exchange:visible');
      await expect(exchanges).toHaveCount(3, { timeout: 10_000 });
      const resumeExchange = exchanges.nth(2);
      const card = resumeExchange.locator('.event-row[data-kind="resume"]');
      await card.locator('.event-row-fold-toggle').click();
      await expect(card.getByText('Lucidos reminded the agent of the 2 steps it had already taken.')).toBeVisible();
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});

test.describe('Question-parked thread preserved across restart', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('answerable card, no Aborted/Restarted/Continue boundary', async ({ page }) => {
    const threadId = randomUUID();
    const userMsgId = randomUUID();
    const askId = randomUUID();
    const t0 = new Date(Date.now() - 2_000).toISOString();
    const t1 = new Date(Date.now() - 1_000).toISOString();

    const userPayload = JSON.stringify({ text: 'make a story', channel: 'chat' });
    const askPayload = JSON.stringify({
      tool_use_id: 'toolu_e2e#q0',
      cc_session_id: '',
      question: 'Which illustration?',
      options: [
        { id: 'opt-0', label: 'Approve the image', description: 'Use it' },
        { id: 'opt-1', label: 'Make a new one', description: 'Regenerate' },
      ],
      multi_select: false,
      channel: 'chat',
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Preserved question e2e', 'chat', '${t1}', 1, false, false, 'waiting_for_user_answer', 'inbox', false, 0, 'none', false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '${userPayload}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${askId}', 'UserQuestionAsked', '${askPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      await expect(page.locator('.question-body').first()).toBeVisible({ timeout: 10_000 });
      await expect(page.locator('.question-body-terminated')).toHaveCount(0);
      const approve = page.locator('.question-option', { hasText: 'Approve the image' }).first();
      await expect(approve).toBeVisible();
      await expect(approve).toBeEnabled();

      await expect(page.getByRole('button', { name: 'Continue' })).toHaveCount(0);
      await expect(page.getByText('Response interrupted')).toHaveCount(0);
      await expect(page.locator('.event-row-state', { hasText: 'Paused by restart' })).toHaveCount(0);
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
