import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, isMobileViewport } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * The composer's Send takes ONE tap with the mobile keyboard up.
 *
 * Reported from a phone: the round blue Send arrow did nothing. The text stayed
 * in the box and no toast appeared, so the press never reached the handler.
 * Tapping a button while a text field is focused can blur the field. The
 * keyboard dismissal then moves the button out from under the finger, before
 * WebKit dispatches the synthetic click, and the click is dropped.
 *
 * The button now also runs on `touchend`, inside the gesture. This pins the two
 * halves an emulator CAN see: a touch tap sends, and it sends exactly once. The
 * second is the hazard the fix introduces, since a browser that dispatches the
 * suppressed click anyway would otherwise send twice.
 *
 * It cannot reproduce the original failure. Playwright's WebKit dispatches the
 * click real Safari drops, which is why the decision itself is unit-tested in
 * `src/utils/tapGesture.test.ts` and the wiring in `prompt-cancel-tap-gate.test.ts`.
 */

/** Visual-viewport height an open iOS keyboard leaves on this device, which is
 *  what MobileSwipeContainer writes to `--app-height`. */
const KEYBOARD_APP_HEIGHT_PX = 490;

/** Long enough to outlast the twin window in `touchActivated`, so a late
 *  synthetic click would have landed by the time the count is re-read. */
const LATE_CLICK_GRACE_MS = 1_500;

test.describe('Composer Send with the mobile keyboard up', () => {
  // iPhone 15 Pro portrait points, the device the report came from.
  test.use({ viewport: { width: 393, height: 852 } });

  test('sends on one touch tap, and only once', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const title = `E2E Send ${suffix}`;
    const followUp = `one tap only ${suffix}`;
    const now = new Date().toISOString();

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', '${title}', 'chat', '${now}', 1, false, true, 'idle', 'inbox', 'active', false, 0, 0, false, false, false)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"seed","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Seeded.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const sentCount = () => psql(
      `SELECT count(*) FROM events WHERE thread_id = '${threadId}'`
      + ` AND event_type = 'MessageReceived' AND payload->>'text' = '${followUp}'`,
    );

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("${title}")`).first().click();
      await ensureOnThreadPane(page);

      // The reported conditions: the user's ui scale, and the app shell shrunk
      // to what an open keyboard leaves of the visual viewport.
      await page.evaluate((h: number) => {
        document.documentElement.style.setProperty('--user-ui-scale', '112.5%');
        document.documentElement.style.setProperty('--app-height', `${h}px`);
      }, KEYBOARD_APP_HEIGHT_PX);

      const input = page.locator('[data-role="prompt-input"]:visible').first();
      await input.focus();
      await input.fill(followUp);
      await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');

      const send = page.locator('button[aria-label="Send message"]:visible').first();
      await expect(send).toBeVisible({ timeout: 10_000 });

      const probe = await page.evaluate(() => {
        const btn = Array.from(document.querySelectorAll<HTMLElement>('button[aria-label="Send message"]'))
          .find(b => b.getBoundingClientRect().width > 0);
        if (!btn) return null;
        const r = btn.getBoundingClientRect();
        const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        const shell = document.querySelector('.app-shell') as HTMLElement;
        return {
          rect: { top: r.top, bottom: r.bottom, left: r.left, right: r.right },
          pointerEvents: getComputedStyle(btn).pointerEvents,
          centreIsSend: hit === btn || btn.contains(hit),
          shellBottom: shell.getBoundingClientRect().bottom,
        };
      });

      expect(probe, 'the Send button never rendered').not.toBeNull();
      expect(probe!.pointerEvents, 'Send is inert while the keyboard is up').not.toBe('none');
      expect(probe!.centreIsSend, 'something else answers the pointer at the centre of Send').toBe(true);
      expect(probe!.rect.bottom, 'Send sits below the app shell, behind the keyboard')
        .toBeLessThanOrEqual(probe!.shellBottom + 0.5);

      // Chromium injects real touch input over CDP, so the tap itself is
      // testable there. WebKit exposes no equivalent on a mobile context.
      if (browserName !== 'chromium') return;

      const r = probe!.rect;
      const cdp = await page.context().newCDPSession(page);
      await cdp.send('Input.dispatchTouchEvent', {
        type: 'touchStart',
        touchPoints: [{ x: Math.round((r.left + r.right) / 2), y: Math.round((r.top + r.bottom) / 2) }],
      });
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });

      // `psql` runs tuples-only and trims, so the count is the whole reply and
      // an exact match is honest. A loose one would read a 2 as a pass.
      await expect
        .poll(sentCount, { timeout: 15_000, message: 'a touch tap on Send never produced a message' })
        .toBe('1');
      await expect(input, 'the draft survived the send').toHaveValue('');

      // The tap must not also arrive as a click. A double send is the failure
      // this half guards, and it would land after the twin window closes.
      await page.waitForTimeout(LATE_CLICK_GRACE_MS);
      expect(sentCount(), 'one tap sent the message twice').toBe('1');
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
