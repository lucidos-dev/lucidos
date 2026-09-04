import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, navigateToApp } from './helpers';
import { psql } from './db-helpers';

test.describe('Error display states', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('malformed chat stream request shows error response', async ({ page }) => {
    // Send a request with missing required fields
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
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'Recovery rerun e2e', 'chat', '${t3}', 1, false, true, 'idle', 'archived', false, 0, false, false, false)`,
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
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'Engine abort e2e', 'chat', '${t1}', 1, false, false, 'idle', 'inbox', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '${userPayload}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      // The abort boundary opens its own exchange whose initiator label is
      // the engine label ("Lucidos Engine", with the Lucidos mark glyph). The
      // Continue button sits in the initiator footer.
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

  test('chat thread aborted by /api/v1/restart: iconless action label, no Continue button', async ({ page }) => {
    const threadId = randomUUID();
    const userMsgId = randomUUID();
    const abortedId = randomUUID();
    const t0 = new Date(Date.now() - 3_000).toISOString();
    const t1 = new Date(Date.now() - 2_000).toISOString();

    const userPayload = JSON.stringify({ text: 'fix the bug', channel: 'chat' });
    // Both halves of the switch-teardown fingerprint, because
    // `isSwitchTeardownAbort` requires both: the `engine_shutdown` cause AND
    // the device that clicked Switch. A device actor alone is not it, since
    // `stale_settle` also carries the actor of the user button that exposed the
    // stuck row, so an actor-only fixture reads as a plain interruption.
    const abortedPayload = JSON.stringify({
      text: '',
      images: [],
      request_event_id: userMsgId,
      cause: 'engine_shutdown',
      actor: { kind: 'device', device_id: 'd-test', label: 'My Mac' },
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'Device abort e2e', 'chat', '${t1}', 1, false, false, 'idle', 'inbox', false, 0, false, false, false)`,
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
      // Device-driven abort (you hit Restart) renders like the ResponseCanceled
      // boundary: iconless, the action AS the label ("Paused by restart"), no
      // "You" chip; who/what is in the timestamp popover.
      await expect(abortExchange.locator('.initiator-icon')).toHaveCount(0);
      await expect(abortExchange.locator('.initiator-label')).toContainText('Paused by restart');
      // And NO Continue button, which is the point of the "paused" wording: the
      // engine promised to resume this turn itself (ADR 0045), so offering a
      // Continue would invite the user to run it a second time.
      // `continuableAbortIndex` withholds it on exactly the fingerprint that
      // produces the label above, so the two can never contradict each other.
      // If the boot declines to resume, it emits a fresh `recovery_after_restart`
      // abort, which does not match, and the button comes back.
      await expect(abortExchange.getByRole('button', { name: 'Continue' })).toHaveCount(0);
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('Continue button hides after a ContinuationStarted lands', async ({ page }) => {
    // Same shape as the engine-abort test, but with a trailing ContinuationStarted
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
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'Resumed e2e', 'chat', '${t2}', 1, false, false, 'idle', 'archived', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '{"text":"hi","channel":"chat"}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${recoveredId}', 'ContinuationStarted', '${recoveredPayload}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      // Three exchanges: original, abort boundary, resume.
      const exchanges = page.locator('.chat-exchange:visible');
      await expect(exchanges).toHaveCount(3, { timeout: 10_000 });
      // The abort boundary's Continue button must NOT render once a
      // ContinuationStarted exists later in the thread.
      const abortExchange = exchanges.nth(1);
      await expect(abortExchange.getByRole('button', { name: 'Continue' })).toHaveCount(0);
      // The resume exchange shows the device-attributed initiator.
      // The summary text must NOT say "engine restart" — this is a
      // user-clicked Continue, the engine was never restarted.
      const resumeExchange = exchanges.nth(2);
      // The resume turn (you clicked Continue) renders like the cancel boundary:
      // iconless, the action AS the label. It must read "Continued the response",
      // NOT "...engine restart", and carry no "You" chip.
      await expect(resumeExchange.locator('.initiator-icon')).toHaveCount(0);
      await expect(resumeExchange.locator('.initiator-label')).toContainText('Continued the response');
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
      origin: { kind: 'engine', reason: { kind: 'continuation_started' } },
      request_event_id: recoveredId,
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'Engine note e2e', 'chat', '${t3}', 1, false, false, 'idle', 'archived', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '{"text":"hi","channel":"chat"}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${abortedId}', 'ResponseAborted', '${abortedPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${recoveredId}', 'ContinuationStarted', '${recoveredPayload}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
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

// ---------------------------------------------------------------------------
// Question-parked threads are PRESERVED across a restart (no abort): the card
// stays answerable, and NO "Aborted / Restarted / Response interrupted /
// Continue" boundary appears. This is the inverse of the boundary panels above
// and the exact user-reported screenshots. The backend guard
// (`thread_has_unanswered_question` gating every restart-abort path) is proven
// by the engine integration tests; this seeds the post-restart state that guard
// now guarantees — a `UserQuestionAsked` with NO trailing `ResponseAborted`,
// status `waiting_for_user_answer` — and asserts the user-visible contract.
// ---------------------------------------------------------------------------
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
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'Preserved question e2e', 'chat', '${t1}', 1, false, false, 'waiting_for_user_answer', 'inbox', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${userMsgId}', 'MessageReceived', '${userPayload}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${askId}', 'UserQuestionAsked', '${askPayload}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      // The question card renders ANSWERABLE (live `.question-body`, enabled
      // options) — NOT the disabled `.question-body-terminated` an abort/overtake
      // would produce.
      await expect(page.locator('.question-body').first()).toBeVisible({ timeout: 10_000 });
      await expect(page.locator('.question-body-terminated')).toHaveCount(0);
      const approve = page.locator('.question-option', { hasText: 'Approve the image' }).first();
      await expect(approve).toBeVisible();
      await expect(approve).toBeEnabled();

      // No restart/abort boundary anywhere in the thread.
      await expect(page.getByRole('button', { name: 'Continue' })).toHaveCount(0);
      await expect(page.getByText('Response interrupted')).toHaveCount(0);
      await expect(page.locator('.initiator-label', { hasText: 'Paused by restart' })).toHaveCount(0);
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
