import { test, expect } from '@playwright/test';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * Browser e2e for CC AskUserQuestion interactive UI.
 *
 * We inject a synthetic UserQuestionAsked event directly into the DB for a
 * CC thread (mirroring what the engine would do after intercepting CC's
 * AskUserQuestion tool_use). UserQuestionAsked starts its OWN exchange — the
 * QuestionBody renders as the body of an initiator panel (the asking agent's
 * chip — "Claude Code" in a CC thread), NOT inline in the prior CC response
 * panel. The browser must:
 *   - Render a divider initiator panel containing the question + options.
 *   - Persist a UserQuestionAnswered event after the user clicks an option.
 *   - Flip the SAME panel in place to its answered state (selected option
 *     highlighted, others dimmed) — no new panel materializes for the click.
 *
 * Spawning a real CC subprocess that emits AskUserQuestion is out of scope
 * for browser e2e — the parser-level wiring is covered by Rust unit tests.
 */
test.describe('CC AskUserQuestion — interactive answer flow', () => {
  test('clicking an option flips the divider initiator panel in place', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const toolUseId = `tu-e2e-${suffix}`;
    const now = new Date().toISOString();

    // Seed a CC thread + UserQuestionAsked event.
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

      // Open the seeded thread.
      const row = page.locator(`.thread-row:has-text("CC Question E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      // The QuestionBody lives inside the initiator panel of the divider
      // exchange. Both SplitLayout (desktop) and MobileSwipeContainer (mobile)
      // render simultaneously, so we scope to the visible copy — `.first()`
      // would otherwise pick the hidden one. We locate the panel by its
      // unique question text, not by `data-tool-use-id`, because the answered
      // body drops the data attribute when it flips to its resolved render.
      const panel = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Pick option ${suffix}"))`)
        .first();
      await expect(panel).toBeVisible({ timeout: 10_000 });
      // Chip on the divider reads the asking agent — "Claude Code" in a CC thread.
      await expect(panel.locator('.initiator-label')).toHaveText('Claude Code');
      // Pending body carries `data-tool-use-id` and exposes the option buttons.
      const pendingBody = panel.locator(`.initiator-body .question-body[data-tool-use-id="${toolUseId}"]`).first();
      await expect(pendingBody).toBeVisible();
      await expect(pendingBody).toContainText(`Yes ${suffix}`);
      await expect(pendingBody).toContainText(`No ${suffix}`);

      // Click the second option.
      await pendingBody.locator('.question-option').nth(1).click();

      // The DB should have a UserQuestionAnswered for this tool_use_id.
      await expect.poll(
        () => psql(`SELECT COUNT(*) FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = '${toolUseId}'`),
        { intervals: [400], timeout: 10_000 },
      ).toBe('1');

      // The SAME panel flips in place to its answered state. We re-locate the
      // answered body inside the same panel (matched by question text) so we
      // prove no new initiator panel materialized for the click.
      const answered = panel.locator('.initiator-body .question-body-answered').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      // The picked option is highlighted; the other is dimmed.
      const selectedOption = answered.locator('.question-option-selected');
      await expect(selectedOption).toHaveCount(1);
      await expect(selectedOption).toContainText(`No ${suffix}`);
      await expect(answered.locator('.question-option-dimmed')).toHaveCount(1);

      // Exactly one divider initiator panel for this question — flipping
      // happens in-place, no duplicate panel materialized.
      await expect(
        page.locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Pick option ${suffix}"))`),
      ).toHaveCount(1);
    } finally {
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

    // Two separate UserQuestionAsked events in the same CC turn (MP1) — each
    // should be its own divider panel, not coalesced.
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

      // Each question gets its own divider initiator panel. We locate by the
      // unique question text rather than `data-tool-use-id`, because the
      // answered body strips the data attribute when it flips state.
      const panel1 = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("First question ${suffix}"))`)
        .first();
      const panel2 = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Second question ${suffix}"))`)
        .first();
      await expect(panel1).toBeVisible({ timeout: 10_000 });
      await expect(panel2).toBeVisible({ timeout: 10_000 });

      // Panel 1 already has its answer applied (Selected → answered body).
      await expect(panel1.locator('.initiator-body .question-body-answered')).toBeVisible();
      // Panel 2 is still pending — the pending body carries `data-tool-use-id`
      // and the clickable option button is present.
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

  test('multi-select: toggle two options, submit, panel flips to multi-resolved', async ({ page }) => {
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

      // Submit lives in the prompt action row now (PromptInput.tsx) — there's
      // one prompt rendered per layout (desktop + mobile both mount), so scope
      // by visibility. Disabled with zero selections + empty textarea.
      const submit = page.locator('.prompt-actions-row:visible button[aria-label="Submit answer"]').first();
      await expect(submit).toBeVisible();
      await expect(submit).toBeDisabled();

      // Toggle Red and Green (skip Blue).
      await pendingBody.locator('.question-option').nth(0).click();
      await pendingBody.locator('.question-option').nth(2).click();
      await expect(submit).toBeEnabled();
      await expect(pendingBody.locator('.question-option[aria-pressed="true"]')).toHaveCount(2);

      // Toggle Red off — back to one.
      await pendingBody.locator('.question-option').nth(0).click();
      await expect(pendingBody.locator('.question-option[aria-pressed="true"]')).toHaveCount(1);

      // Re-toggle Red, then Submit.
      await pendingBody.locator('.question-option').nth(0).click();
      await submit.click();

      // DB: UserQuestionAnswered with MultiSelected[opt-0, opt-2].
      await expect.poll(
        () => psql(`SELECT payload->'answer' FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = '${toolUseId}'`),
        { intervals: [400], timeout: 10_000 },
      ).toContain('MultiSelected');

      // Panel flips in place: Red and Green selected, Blue dimmed.
      const answered = panel.locator('.initiator-body .question-body-answered').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      await expect(answered.locator('.question-option-selected')).toHaveCount(2);
      await expect(answered.locator('.question-option-dimmed')).toHaveCount(1);
    } finally {
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

    // Seed a question + a Canceled answer (the engine writes this when the
    // user hits Stop while CC is paused on AskUserQuestion). `has_response,
    // true` keeps the row visible in the drawer — `get_recent_threads` filters
    // out idle threads with no response, so the seed must claim a response.
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

      // Locate by question text — the answered body has no `data-tool-use-id`.
      const panel = page
        .locator(`.initiator-panel-lucidos:visible:has(.question-text:has-text("Cancel question ${suffix}"))`)
        .first();
      await expect(panel).toBeVisible({ timeout: 10_000 });

      const answered = panel.locator('.initiator-body .question-body-answered').first();
      await expect(answered).toBeVisible();
      // All options dimmed; nothing selected.
      await expect(answered.locator('.question-option-selected')).toHaveCount(0);
      await expect(answered.locator('.question-option-dimmed')).toHaveCount(2);
      // Cancel renders as a disabled red Cancel button styled like the picked
      // permission affordance — assert via class, not text, so the assertion
      // survives copy edits.
      await expect(answered.locator('.question-cancel-picked')).toBeVisible();
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
