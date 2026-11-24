import { test, expect, type APIRequestContext } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';


function placeholderOverflowPx(el: Element): number {
  const ta = el as HTMLTextAreaElement;
  const probe = ta.cloneNode() as HTMLTextAreaElement;
  probe.value = ta.placeholder;
  probe.removeAttribute('data-role');
  Object.assign(probe.style, {
    position: 'absolute', visibility: 'hidden', boxSizing: 'border-box',
    width: `${ta.getBoundingClientRect().width}px`,
    height: '0', minHeight: '0', maxHeight: 'none',
  });
  ta.parentElement!.appendChild(probe);
  const needed = probe.scrollHeight;
  probe.remove();
  return Math.max(0, needed - ta.clientHeight);
}

async function endResumedSession(request: APIRequestContext, threadId: string): Promise<void> {
  const answered = psql(`SELECT COUNT(*) FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered'`);
  if (answered === '0') return;
  const live = async () => {
    const res = await request.get(`/api/v1/claude-code/commands?thread_id=${threadId}`);
    expect(res.ok(), `commands for thread ${threadId}: HTTP ${res.status()}`).toBe(true);
    return (await res.json()).has_active_session === true;
  };
  const endedAfterAnswer = () => psql(
    `SELECT COUNT(*) FROM events e JOIN events a ON a.thread_id = e.thread_id
     WHERE a.thread_id = '${threadId}' AND a.event_type = 'UserQuestionAnswered' AND e.created > a.created
       AND e.event_type IN ('CodingAgentIdled', 'ResponseCanceled', 'ResponseAborted', 'ResponseFailed')`,
  ) !== '0';
  await expect.poll(async () => (await live()) || endedAfterAnswer(), {
    message: `the session the answer resumed on thread ${threadId} neither started nor ended`,
    intervals: [250],
    timeout: 30_000,
  }).toBe(true);
  if (await live()) {
    const stop = await request.post(`/api/v1/claude-code/stop?thread_id=${threadId}&discard=true`);
    expect([200, 404], `stop thread ${threadId}: HTTP ${stop.status()}`).toContain(stop.status());
  }
  await expect.poll(live, {
    message: `the session the answer resumed on thread ${threadId} is still live after the stop`,
    intervals: [250],
    timeout: 30_000,
  }).toBe(false);
}

test.describe('CC AskUserQuestion — interactive answer flow', () => {
  test('clicking an option flips the divider initiator panel in place', async ({ page, request }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const toolUseId = `tu-e2e-${suffix}`;
    const now = new Date().toISOString();

    const msgEventId = randomUUID();
    const sessionStartedId = randomUUID();
    const questionEventId = randomUUID();
    const payload = JSON.stringify({
      tool_use_id: toolUseId,
      cc_session_id: 'sess-e2e',
      question: `Pick option ${suffix}`,
      options: [
        { id: 'opt-0', label: `Yes ${suffix}` },
        { id: 'opt-1', label: `No ${suffix}` },
      ],
    }).replace(/'/g, "''");

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Question E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'waiting_for_user_answer', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"start","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${sessionStartedId}', 'SessionStarted', '{"session_id":"sess-e2e"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${questionEventId}', 'UserQuestionAsked', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Question E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const panel = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Pick option ${suffix}"))`)
        .first();
      await expect(panel).toBeVisible({ timeout: 10_000 });
      await expect(panel.locator('.initiator-label')).toHaveText('Claude');
      const pendingBody = panel.locator(`.initiator-body .question-body[data-tool-use-id="${toolUseId}"]`).first();
      await expect(pendingBody).toBeVisible();
      await expect(pendingBody).toContainText(`Yes ${suffix}`);
      await expect(pendingBody).toContainText(`No ${suffix}`);

      await expect(pendingBody.locator('.question-hint')).toHaveCount(0);
      await expect(pendingBody.locator('.question-option')).toHaveCount(2);
      const promptInput = page.locator('[data-role="prompt-input"]:visible').first();
      await expect(promptInput).toHaveAttribute('placeholder', 'Type custom answer here…');
      await expect.poll(
        () => promptInput.evaluate(placeholderOverflowPx), { intervals: [200], timeout: 5_000 },
      ).toBe(0);

      await pendingBody.locator('.question-option').nth(1).click();

      await expect.poll(
        () => psql(`SELECT COUNT(*) FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = '${toolUseId}'`),
        { intervals: [400], timeout: 10_000 },
      ).toBe('1');

      const answered = panel.locator('.initiator-body .question-body-answered').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      const selectedOption = answered.locator('.question-option-selected');
      await expect(selectedOption).toHaveCount(1);
      await expect(selectedOption).toContainText(`No ${suffix}`);
      await expect(answered.locator('.question-option-dimmed')).toHaveCount(1);
      await expect(promptInput).toHaveAttribute('placeholder', 'Post a follow up…');
      await expect.poll(
        () => promptInput.evaluate((el) => {
          const ta = el as HTMLTextAreaElement;
          const cs = getComputedStyle(ta);
          const content = ta.clientHeight
            - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom);
          return content < parseFloat(cs.lineHeight) * 2;
        }),
        { intervals: [200], timeout: 5_000 },
      ).toBe(true);

      await expect(
        page.locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Pick option ${suffix}"))`),
      ).toHaveCount(1);
    } finally {
      await endResumedSession(request, threadId);
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('multi-pause CC turn renders one divider initiator panel per question', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const tool1 = `tu1-e2e-${suffix}`;
    const tool2 = `tu2-e2e-${suffix}`;
    const base = Date.now();
    const stamp = (offsetMs: number) => new Date(base + offsetMs).toISOString();

    const payload1 = JSON.stringify({
      tool_use_id: tool1,
      cc_session_id: 'sess-multi-e2e',
      question: `First question ${suffix}`,
      options: [{ id: 'opt-0', label: `A ${suffix}` }],
    }).replace(/'/g, "''");
    const payload2 = JSON.stringify({
      tool_use_id: tool2,
      cc_session_id: 'sess-multi-e2e',
      question: `Second question ${suffix}`,
      options: [{ id: 'opt-0', label: `B ${suffix}` }],
    }).replace(/'/g, "''");
    const answer1 = JSON.stringify({
      tool_use_id: tool1,
      answer: { kind: 'Selected', option_id: 'opt-0' },
    }).replace(/'/g, "''");

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Multi Question E2E ${suffix}', 'claude_code', '${stamp(0)}', 1, false, false, 'waiting_for_user_answer', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"do stuff","channel":"claude_code"}'::jsonb, '${stamp(0)}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-multi-e2e"}'::jsonb, '${stamp(10)}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${payload1}'::jsonb, '${stamp(20)}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAnswered', '${answer1}'::jsonb, '${stamp(30)}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${payload2}'::jsonb, '${stamp(40)}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Multi Question E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const panel1 = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("First question ${suffix}"))`)
        .first();
      const panel2 = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Second question ${suffix}"))`)
        .first();
      await expect(panel1).toBeVisible({ timeout: 10_000 });
      await expect(panel2).toBeVisible({ timeout: 10_000 });

      await expect(panel1.locator('.initiator-body .question-body-answered')).toBeVisible();
      await expect(panel2.locator('.initiator-body .question-body-answered')).toHaveCount(0);
      await expect(panel2.locator(`.question-body[data-tool-use-id="${tool2}"]`)).toBeVisible();
      await expect(panel2.locator('.question-option')).toHaveCount(1);
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('multi-select: toggle two options, submit, panel flips to multi-resolved', async ({ page, request }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const toolUseId = `tu-multi-e2e-${suffix}`;
    const now = new Date().toISOString();

    const payload = JSON.stringify({
      tool_use_id: toolUseId,
      cc_session_id: 'sess-multi-e2e',
      question: `Multi pick ${suffix}`,
      options: [
        { id: 'opt-0', label: `Red ${suffix}` },
        { id: 'opt-1', label: `Blue ${suffix}` },
        { id: 'opt-2', label: `Green ${suffix}` },
      ],
      multi_select: true,
    }).replace(/'/g, "''");

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Multi Select E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'waiting_for_user_answer', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"start","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-multi-e2e"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Multi Select E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const panel = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Multi pick ${suffix}"))`)
        .first();
      await expect(panel).toBeVisible({ timeout: 10_000 });
      const pendingBody = panel.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first();

      const submit = page.locator('.prompt-actions-row:visible button[aria-label="Submit answer"]').first();
      await expect(page.locator('.prompt-actions-row:visible button[aria-label="Cancel"]')).toBeVisible();
      await expect(submit).toHaveCount(0);

      await pendingBody.locator('.question-option').nth(0).click();
      await pendingBody.locator('.question-option').nth(2).click();
      await expect(submit).toBeEnabled();
      await expect(pendingBody.locator('.question-option[aria-pressed="true"]')).toHaveCount(2);

      await pendingBody.locator('.question-option').nth(0).click();
      await expect(pendingBody.locator('.question-option[aria-pressed="true"]')).toHaveCount(1);

      await pendingBody.locator('.question-option').nth(0).click();
      await submit.click();

      await expect.poll(
        () => psql(`SELECT payload->'answer' FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = '${toolUseId}'`),
        { intervals: [400], timeout: 10_000 },
      ).toContain('MultiSelected');

      const answered = panel.locator('.initiator-body .question-body-answered').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      await expect(answered.locator('.question-option-selected')).toHaveCount(2);
      await expect(answered.locator('.question-option-dimmed')).toHaveCount(1);
    } finally {
      await endResumedSession(request, threadId);
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('canceled answer dims options and shows the Canceled badge', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const toolUseId = `tu-cancel-e2e-${suffix}`;
    const base = Date.now();
    const stamp = (offsetMs: number) => new Date(base + offsetMs).toISOString();

    const payload = JSON.stringify({
      tool_use_id: toolUseId,
      cc_session_id: 'sess-cancel-e2e',
      question: `Cancel question ${suffix}`,
      options: [
        { id: 'opt-0', label: `Yes ${suffix}` },
        { id: 'opt-1', label: `No ${suffix}` },
      ],
    }).replace(/'/g, "''");
    const answer = JSON.stringify({
      tool_use_id: toolUseId,
      answer: { kind: 'Canceled' },
    }).replace(/'/g, "''");

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Cancel Question E2E ${suffix}', 'claude_code', '${stamp(0)}', 1, false, true, 'idle', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"start","channel":"claude_code"}'::jsonb, '${stamp(0)}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-cancel-e2e"}'::jsonb, '${stamp(10)}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${payload}'::jsonb, '${stamp(20)}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAnswered', '${answer}'::jsonb, '${stamp(30)}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Cancel Question E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const panel = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Cancel question ${suffix}"))`)
        .first();
      await expect(panel).toBeVisible({ timeout: 10_000 });

      const answered = panel.locator('.initiator-body .question-body-answered').first();
      await expect(answered).toBeVisible();
      await expect(answered.locator('.question-option-selected')).toHaveCount(0);
      await expect(answered.locator('.question-option-dimmed')).toHaveCount(2);
      await expect(answered.locator('.question-cancel-picked')).toBeVisible();
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('Escape in the prompt cancels the pending question', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const toolUseId = `tu-esc-e2e-${suffix}`;
    const now = new Date().toISOString();

    const payload = JSON.stringify({
      tool_use_id: toolUseId,
      cc_session_id: 'sess-esc-e2e',
      question: `Escape question ${suffix}`,
      options: [{ id: 'opt-0', label: `Yes ${suffix}` }],
    }).replace(/'/g, "''");

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Escape Question E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'waiting_for_user_answer', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"start","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-esc-e2e"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Escape Question E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const panel = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Escape question ${suffix}"))`)
        .first();
      await expect(panel).toBeVisible({ timeout: 10_000 });

      const promptInput = page.locator('[data-role="prompt-input"]:visible').first();
      await promptInput.focus();
      await promptInput.press('Escape');

      await expect.poll(
        () => psql(`SELECT payload->'answer'->>'kind' FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = '${toolUseId}'`),
        { intervals: [400], timeout: 10_000 },
      ).toBe('Canceled');
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('first option takes visible keyboard focus, arrows step, Enter answers', async ({ page, request }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const toolUseId = `tu-kbd-e2e-${suffix}`;
    const now = new Date().toISOString();

    const payload = JSON.stringify({
      tool_use_id: toolUseId,
      cc_session_id: 'sess-kbd-e2e',
      question: `Keyboard pick ${suffix}`,
      options: [
        { id: 'opt-0', label: `First ${suffix}` },
        { id: 'opt-1', label: `Second ${suffix}` },
        { id: 'opt-2', label: `Third ${suffix}` },
      ],
    }).replace(/'/g, "''");

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'CC Question Keyboard E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'waiting_for_user_answer', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"start","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-kbd-e2e"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("CC Question Keyboard E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const options = page
        .locator(`.question-body[data-tool-use-id="${toolUseId}"]:visible .question-option`);
      await expect(options).toHaveCount(3, { timeout: 10_000 });

      await expect(options.nth(0)).toBeFocused({ timeout: 10_000 });

      const ring = await page.evaluate(() => {
        const el = document.activeElement as HTMLElement | null;
        return el ? getComputedStyle(el).boxShadow : '';
      });
      expect(ring).not.toBe('');
      expect(ring).not.toBe('none');

      await page.keyboard.press('ArrowDown');
      await expect(options.nth(1)).toBeFocused();
      await page.keyboard.press('ArrowUp');
      await expect(options.nth(0)).toBeFocused();
      await page.keyboard.press('ArrowUp');
      await expect(options.nth(0)).toBeFocused();
      await page.keyboard.press('ArrowDown');
      await page.keyboard.press('ArrowDown');
      await expect(options.nth(2)).toBeFocused();

      await page.keyboard.press('Enter');

      await expect.poll(
        () => psql(`SELECT payload->'answer'->>'option_id' FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = '${toolUseId}'`),
        { intervals: [400], timeout: 10_000 },
      ).toBe('opt-2');
    } finally {
      await endResumedSession(request, threadId);
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
