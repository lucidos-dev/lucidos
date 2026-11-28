import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, waitForPaneAtRest, isMobileViewport } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';


const OPTIONS = Array.from({ length: 8 }, (_, i) => ({
  id: `opt-${i}`,
  label: `Area ${i + 1}: something worth picking`,
  description: 'A description long enough to wrap onto a second line at phone width, as the real cards do.',
}));

test.describe('Transcript scroll with a live multi-select card (mobile)', () => {
  test('stays scrollable and reachable while the composer is focused', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const toolUseId = `tu-multi-${suffix}`;
    const now = new Date().toISOString();
    const question = JSON.stringify({
      tool_use_id: toolUseId,
      cc_session_id: '',
      channel: 'chat',
      multi_select: true,
      question: `Which parts should this help with? Pick everything that applies. ${suffix}`,
      options: OPTIONS,
    }).replace(/'/g, "''");
    const message = JSON.stringify({ text: `Start the interview ${suffix}`, channel: 'chat' }).replace(/'/g, "''");

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'Multi-select scroll ${suffix}', 'chat', '${now}', 1, false, true, 'waiting_for_user_answer', 'inbox', false, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '${message}'::jsonb, '${new Date(Date.now() - 5000).toISOString()}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${question}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("Multi-select scroll ${suffix}")`).first().click();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);
      await expect(page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first()).toBeVisible({ timeout: 15_000 });

      const transcript = page.locator('.thread-content.visible:visible').first();
      await expect
        .poll(() => transcript.evaluate(el => el.scrollHeight - el.clientHeight), { timeout: 10_000 })
        .toBeGreaterThan(50);

      await page.locator('[data-role="prompt-input"]:visible').first().focus();
      await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');

      const hit = await page.evaluate(() => {
        const tc = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
          .find(el => el.getBoundingClientRect().height > 0)!;
        const r = tc.getBoundingClientRect();
        const at = (frac: number) => document.elementFromPoint(r.left + r.width / 2, r.top + r.height * frac);
        return {
          scrollerIsHittable: getComputedStyle(tc).pointerEvents !== 'none',
          insideScroller: [0.3, 0.5, 0.7].map(f => {
            const el = at(f);
            return !!el && (el === tc || tc.contains(el));
          }),
          optionInert: getComputedStyle(tc.querySelector('.question-option')!).pointerEvents === 'none',
        };
      });
      expect(hit.scrollerIsHittable, 'the transcript scroller must answer the pointer').toBe(true);
      expect(hit.insideScroller, 'a touch mid-transcript must land on the scroller').toEqual([true, true, true]);
      expect(hit.optionInert, 'a stray tap must still not answer the question').toBe(true);

      if (browserName === 'chromium') {
        const box = (await transcript.boundingBox())!;
        const x = Math.round(box.x + box.width / 2);
        const yFrom = Math.round(box.y + box.height * 0.75);
        const cdp = await page.context().newCDPSession(page);
        const touch = (type: 'touchStart' | 'touchMove' | 'touchEnd', y: number) =>
          cdp.send('Input.dispatchTouchEvent', {
            type,
            touchPoints: type === 'touchEnd' ? [] : [{ x, y }],
          });
        await touch('touchStart', yFrom);
        for (let step = 1; step <= 6; step++) await touch('touchMove', yFrom - step * 30);
        await touch('touchEnd', yFrom - 180);
        await expect
          .poll(() => transcript.evaluate(el => el.scrollTop), { timeout: 5_000 })
          .toBeGreaterThan(0);
      }
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
