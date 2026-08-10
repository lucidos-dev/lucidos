import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, isMobileViewport } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * The transcript must stay scrollable while the composer holds focus.
 *
 * A multi-select question card is the case that makes this load-bearing rather
 * than cosmetic: the card is IN the transcript, its Submit is in the prompt row,
 * and the card invites a typed custom answer alongside the picks. So the reader
 * is routinely typing with a tall card above them, and the options they still
 * have to reach are below the fold.
 *
 * The bug: `:root[data-keyboard-active] .thread-content` was
 * `pointer-events: none`, which took the SCROLLER out of hit-testing. A touch is
 * hit-tested to the deepest element that answers the pointer and then pans that
 * element's nearest scrollable ancestor, so every touch resolved to
 * `.thread-content-wrap`, which scrolls nothing. The transcript froze the moment
 * a textarea took focus. The horizontal pane swipe kept working, because its
 * handler sits on the swipe container above the wrap, which is exactly how it
 * was reported: "swipe up/down did not work, frozen, swipe left/right worked".
 *
 * Blocking the children instead reaches the same "no stray taps" end
 * (`pointer-events` is inherited, so the subtree goes inert) while the pan keeps
 * landing on the scroller. `styles/__tests__/scroller-hit-target-guard.test.ts`
 * pins the declaration; this pins the behaviour.
 */

/** Options tall enough that the card alone overflows a phone viewport, so the
 *  scroll assertions below can never be vacuous. */
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
      await expect(page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first()).toBeVisible({ timeout: 15_000 });

      const transcript = page.locator('.thread-content.visible:visible').first();
      // Not vacuous: the card really does run past the fold.
      await expect
        .poll(() => transcript.evaluate(el => el.scrollHeight - el.clientHeight), { timeout: 10_000 })
        .toBeGreaterThan(50);

      // The reader starts typing a custom answer alongside their picks.
      await page.locator('[data-role="prompt-input"]:visible').first().focus();
      await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');

      // What a finger landing mid-transcript would hit. It must resolve INTO the
      // scroller, or there is nothing for the browser to pan.
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
          // The point of the block is unchanged: nothing in there can be activated.
          optionInert: getComputedStyle(tc.querySelector('.question-option')!).pointerEvents === 'none',
        };
      });
      expect(hit.scrollerIsHittable, 'the transcript scroller must answer the pointer').toBe(true);
      expect(hit.insideScroller, 'a touch mid-transcript must land on the scroller').toEqual([true, true, true]);
      expect(hit.optionInert, 'a stray tap must still not answer the question').toBe(true);

      // Chromium injects real touch input over CDP, so the pan ITSELF is testable
      // there rather than only its precondition. WebKit exposes no equivalent (and
      // no mouse.wheel on a mobile context), so it stops at the hit-test above,
      // which is the half that actually regressed. An `if` rather than a
      // `test.skip`, so the run on WebKit reports what it is: a pass on everything
      // that engine can be asked.
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
