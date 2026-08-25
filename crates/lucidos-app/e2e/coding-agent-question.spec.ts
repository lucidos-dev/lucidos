import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, isMobileViewport } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * Browser e2e for CC AskUserQuestion interactive UI.
 *
 * We inject a synthetic UserQuestionAsked event directly into the DB for a
 * CC thread (mirroring what the engine would do after intercepting CC's
 * AskUserQuestion tool_use). UserQuestionAsked starts its OWN exchange — the
 * QuestionBody renders as the body of an initiator panel (the asking agent's
 * chip, the short "Claude" in a CC thread), NOT inline in the prior CC response
 * panel. The browser must:
 *   - Render a divider initiator panel containing the question + options.
 *   - Persist a UserQuestionAnswered event after the user clicks an option.
 *   - Flip the SAME panel in place to its answered state (selected option
 *     highlighted, others dimmed) — no new panel materializes for the click.
 *
 * Spawning a real CC subprocess that emits AskUserQuestion is out of scope
 * for browser e2e — the parser-level wiring is covered by Rust unit tests.
 */

/** How many pixels of the prompt's PLACEHOLDER are painted past the bottom of
 *  the box, i.e. clipped by its `overflow-y: hidden` (0 = the whole thing is
 *  readable). Runs in the page: renders the placeholder as a value in an
 *  off-layout clone of the real textarea and compares the height that needs
 *  against the height the box has. The card names neither escape any more, so
 *  the sentence in the placeholder is the only thing that does, and it wraps at
 *  phone widths and in a narrowed thread pane. */
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
      // Chip on the divider reads the asking agent. It is the SHORT "Claude" in
      // a CC thread: this row is the app's tightest on a phone, so the coding
      // agent's own name is shortened here and nowhere else (`describeExecutor`).
      await expect(panel.locator('.initiator-label')).toHaveText('Claude');
      // Pending body carries `data-tool-use-id` and exposes the option buttons.
      const pendingBody = panel.locator(`.initiator-body .question-body[data-tool-use-id="${toolUseId}"]`).first();
      await expect(pendingBody).toBeVisible();
      await expect(pendingBody).toContainText(`Yes ${suffix}`);
      await expect(pendingBody).toContainText(`No ${suffix}`);

      // The card is the question and its options, nothing else: no guide line
      // under the answers.
      await expect(pendingBody.locator('.question-hint')).toHaveCount(0);
      await expect(pendingBody.locator('.question-option')).toHaveCount(2);
      // The prompt row is where the two escapes that need no option slot are
      // named: typing (routed to this question as a freetext answer) by this
      // placeholder, and Cancel by the Cancel button's tooltip. Without them
      // named somewhere the agent invents an "Other, I'll type it" option, which
      // just sends that label back as the user's answer. Literal on purpose: an
      // e2e spec can't import from `src/`, so this is the one deliberate
      // duplicate of `PLACEHOLDER_ANSWERING` in `prompt-input-helpers.ts`.
      // Change both together.
      const promptInput = page.locator('[data-role="prompt-input"]:visible').first();
      await expect(promptInput).toHaveAttribute('placeholder', 'Type custom answer here…');
      // The placeholder has to be READABLE, which is not free: a textarea sizes
      // to its VALUE, so `overflow-y: hidden` silently clips a placeholder that
      // wraps, and this is the longest of the three (it wraps in a narrowed
      // thread pane and at large UI scales). Assert the box is as tall as
      // rendering the placeholder as a value would need, which holds on every
      // project rather than pinning a per-viewport number.
      await expect.poll(
        () => promptInput.evaluate(placeholderOverflowPx), { intervals: [200], timeout: 5_000 },
      ).toBe(0);

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
      // Nothing left to answer, so the prompt stops inviting one.
      await expect(promptInput).toHaveAttribute('placeholder', 'Post a follow up…');
      // The extra height has to be given back too: it is written as inline px
      // and a placeholder swap never touches the value resizeTextarea keys off,
      // so without the re-measure the composer sits two lines tall with nothing
      // left to answer. Compare the CONTENT box (clientHeight carries the
      // padding) so the same assertion holds on desktop and phone alike.
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

  // Escape inside the composer is the keyboard twin of the red Cancel button:
  // with a question pending it stamps that question `Canceled`, which is how the
  // user steers the agent somewhere else without picking an option. The
  // placeholder is what tells them Cancel is there at all, so the key that
  // performs it has to actually perform it.
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

      // Focus the prompt explicitly: arriving on a parked question seeds focus
      // onto the card's first option instead (threadEntryFocusTarget), and this
      // is about Escape from the COMPOSER.
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

  // The *choice card* keyboard contract (docs/glossary.md, choiceCardNav.ts).
  // Opening a thread already parked on a question must land focus on the card
  // rather than the prompt. That is `threadEntryFocusTarget` winning over the
  // prompt's own thread-switch focus, which is the half that races if either
  // side decides independently.
  test('first option takes visible keyboard focus, arrows step, Enter answers', async ({ page }) => {
    test.skip(isMobileViewport(page), 'choice-card keyboard focus is desktop-only (no hardware keyboard on mobile)');
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

      // Seeded on the FIRST option, so Enter answers with no pointer at all.
      await expect(options.nth(0)).toBeFocused({ timeout: 10_000 });

      // ...and the focus is VISIBLE. This is why the CSS pairs an ungated
      // :focus-visible with a hover-gated plain :focus: nothing the user
      // pressed put focus here, and :focus-visible's heuristic alone would
      // leave the ring off exactly when they most need to see what Enter is
      // about to answer.
      const ring = await page.evaluate(() => {
        const el = document.activeElement as HTMLElement | null;
        return el ? getComputedStyle(el).boxShadow : '';
      });
      expect(ring).not.toBe('');
      expect(ring).not.toBe('none');

      // Arrows step, and clamp rather than wrap at the top.
      await page.keyboard.press('ArrowDown');
      await expect(options.nth(1)).toBeFocused();
      await page.keyboard.press('ArrowUp');
      await expect(options.nth(0)).toBeFocused();
      await page.keyboard.press('ArrowUp');
      await expect(options.nth(0)).toBeFocused();
      await page.keyboard.press('ArrowDown');
      await page.keyboard.press('ArrowDown');
      await expect(options.nth(2)).toBeFocused();

      // Enter is the button's own native activation, no key handling of ours.
      await page.keyboard.press('Enter');

      await expect.poll(
        () => psql(`SELECT payload->'answer'->>'option_id' FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = '${toolUseId}'`),
        { intervals: [400], timeout: 10_000 },
      ).toBe('opt-2');
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
