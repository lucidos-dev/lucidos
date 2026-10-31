import { test, expect, Page, Locator } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane } from './helpers';
import { psql } from './db-helpers';

/**
 * Authored HTML stays inside its own box (ADR 0114 and its amendment,
 * `.authored-html` in host-components.css). A reply or a question card that
 * writes `position: fixed`, or borrows a full-screen host class, must never
 * cover the header, the composer or an option. A form must not render, and
 * nothing may reach the top layer.
 *
 * Synthetic payloads only. The form posts to the discard port on loopback, and
 * the test types nothing into it.
 */

const FIXED_COVER =
  '<div style="position:fixed;inset:0;z-index:2147483647;background:red">fixed cover</div>';
const HOST_CLASSES = '<div class="modal-overlay">modal overlay</div>\n\n<div class="pairing-gate">pairing gate</div>';
const FORM =
  '<form action="http://127.0.0.1:9/collect" method="post" style="position:fixed;inset:0;z-index:2147483647;background:white"><input type="password" name="api_key"><button>Connect</button></form>';
const POPOVER =
  '<button popovertarget="e2e-pop">open popover</button><div popover id="e2e-pop" style="position:fixed;inset:0">popover</div>';

const REPLY = ['Contained payloads follow.', FIXED_COVER, HOST_CLASSES, FORM, POPOVER].join('\n\n');

const sql = (value: unknown) => JSON.stringify(value).replace(/'/g, "''");

function insertEvent(threadId: string, type: string, payload: unknown, at: string): string {
  return `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
    `VALUES ('${randomUUID()}', '${type}', '${sql(payload)}'::jsonb, '${at}', 'thread', '${threadId}', '${threadId}')`;
}

function seedReply(title: string): string {
  const threadId = randomUUID();
  const messageId = randomUUID();
  const now = new Date().toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) ` +
      `VALUES ('${threadId}', '${title}', 'chat', '${now}', 1, false, true, 'idle', 'inbox', false, 0)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
      `VALUES ('${messageId}', 'MessageReceived', '{"text":"show me","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    insertEvent(threadId, 'TextStreamed', { text: REPLY, request_event_id: messageId }, now),
    insertEvent(threadId, 'ResponseGenerated', { text: REPLY, images: [], request_event_id: messageId }, now),
  ].join(';\n'));
  return threadId;
}

function seedQuestion(title: string): string {
  const threadId = randomUUID();
  const now = new Date().toISOString();
  const question = {
    tool_use_id: `tu-${threadId.slice(0, 8)}`,
    cc_session_id: 'sess-e2e',
    question: `Pick one.\n\n${FIXED_COVER}\n\n${HOST_CLASSES}`,
    options: [
      { id: 'opt-0', label: 'First choice', description: FIXED_COVER, preview: '<span class="modal-overlay">preview</span>' },
      { id: 'opt-1', label: 'Second choice', description: POPOVER },
    ],
  };
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) ` +
      `VALUES ('${threadId}', '${title}', 'claude_code', '${now}', 1, false, false, 'waiting_for_user_answer', 'inbox', true, 0)`,
    insertEvent(threadId, 'MessageReceived', { text: 'start', channel: 'claude_code' }, now),
    insertEvent(threadId, 'SessionStarted', { session_id: 'sess-e2e' }, now),
    insertEvent(threadId, 'UserQuestionAsked', question, now),
  ].join(';\n'));
  return threadId;
}

async function openThread(page: Page, title: string): Promise<void> {
  await navigateToApp(page);
  await openThreadDrawer(page);
  const row = page.locator(`.thread-row:has-text("${title}")`).first();
  await expect(row).toBeVisible({ timeout: 10_000 });
  await row.click();
  await ensureOnThreadPane(page);
}

/** Whether the topmost element at the centre of `target` is authored HTML. */
async function authoredOnTop(page: Page, target: Locator): Promise<boolean> {
  const box = await target.boundingBox();
  expect(box, 'the probe target has a box').not.toBeNull();
  const x = box!.x + box!.width / 2;
  const y = box!.y + box!.height / 2;
  return page.evaluate(([px, py]) => document.elementFromPoint(px, py)?.closest('.authored-html') != null, [x, y]);
}

/** Every visible authored container computes its clip, on one block box. */
async function expectAllContained(page: Page): Promise<void> {
  const styles = await page.locator('.authored-html:visible').evaluateAll((els) =>
    els.map((el) => ({ clip: getComputedStyle(el).clipPath, display: getComputedStyle(el).display })));
  expect(styles.length).toBeGreaterThan(0);
  for (const style of styles) {
    expect(style.clip).toMatch(/^inset\(/);
    expect(style.display).not.toBe('inline');
  }
}

/** No popover survives, and tapping its would-be invoker opens nothing. A tap
 *  inside a question option would answer it, so that card skips the tap. */
async function expectNoTopLayer(page: Page, { tap }: { tap: boolean }): Promise<void> {
  await expect(page.locator('.authored-html [popover]')).toHaveCount(0);
  if (tap) await page.locator('.authored-html:visible', { hasText: 'open popover' }).first().click();
  await expect(page.locator(':popover-open')).toHaveCount(0);
}

test.describe('authored HTML stays contained', () => {
  const seeded: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test.afterEach(() => {
    if (seeded.length === 0) return;
    const ids = seeded.splice(0).map((id) => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  test('a reply covers neither the header nor the composer, and renders no form', async ({ page }) => {
    const title = `Contained reply ${randomUUID().slice(0, 8)}`;
    seeded.push(seedReply(title));
    await openThread(page, title);

    const reply = page.locator('.authored-html:visible', { hasText: 'Contained payloads follow.' }).first();
    await expect(reply).toBeVisible({ timeout: 10_000 });
    await expectAllContained(page);

    expect(await authoredOnTop(page, page.locator('.app-header:visible').first()), 'the header').toBe(false);
    expect(await authoredOnTop(page, page.locator('.prompt-box:visible').first()), 'the composer').toBe(false);

    await expect(page.locator('.authored-html form, .authored-html input, .authored-html button')).toHaveCount(0);
    await expect(reply).toContainText('<button>Connect</button>');
    await expectNoTopLayer(page, { tap: true });
  });

  test('a question card keeps its options and the header clear', async ({ page }) => {
    const title = `Contained question ${randomUUID().slice(0, 8)}`;
    seeded.push(seedQuestion(title));
    await openThread(page, title);

    const option = page.locator('.question-option:visible', { hasText: 'Second choice' }).first();
    await expect(option).toBeVisible({ timeout: 10_000 });
    await option.scrollIntoViewIfNeeded();
    await expectAllContained(page);

    expect(await authoredOnTop(page, page.locator('.app-header:visible').first()), 'the header').toBe(false);
    for (const label of ['First choice', 'Second choice']) {
      const target = page.locator('.question-option:visible .question-option-label', { hasText: label }).first();
      expect(await authoredOnTop(page, target), `the "${label}" option`).toBe(false);
    }
    await expectNoTopLayer(page, { tap: false });
  });
});
