import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, isMobileViewport } from './helpers';
import { psql, createCCThreadWithChange, cleanupCCThread } from './db-helpers';
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
 * The repair runs the button on `touchend`, inside the gesture. That path is
 * the ONLY one on iOS with a field focused, so it takes every press it is
 * given. It has twice been given a test to fail instead, and both shipped as a
 * dead Send: a hit test against the button's live rect, then the finger's
 * travel between press and lift.
 *
 * This pins what an emulator CAN see. A touch tap sends. It sends exactly once,
 * which is the hazard the touch path introduces, since a browser dispatching
 * the suppressed click anyway would send twice. And a press that TRAVELS still
 * sends, which is the case both of those tests refused.
 *
 * It cannot reproduce the dropped click itself. Playwright's WebKit dispatches
 * the click real Safari drops, which is why the decision is unit-tested in
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

  // The fifth report, and the first the probe could not speak for. The Send face
  // is rendered from the draft store, and the submit used to gate on the box.
  // The two need only drift apart for the press to die in silence.
  //
  // Assigning `value` fires no input event, which is how the state is built here
  // without reaching into the store. That is not the mechanism on the phone.
  // Nobody knows which side drifted there. It is the same STATE, and the send
  // must survive it either way.
  test('sends the draft when the box has lost it', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    test.skip(browserName !== 'chromium', 'needs CDP touch injection');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const title = `E2E Draft Split ${suffix}`;
    const followUp = `only the draft has this ${suffix}`;
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

      await page.evaluate((h: number) => {
        document.documentElement.style.setProperty('--app-height', `${h}px`);
      }, KEYBOARD_APP_HEIGHT_PX);

      const input = page.locator('[data-role="prompt-input"]:visible').first();
      await input.focus();
      await input.fill(followUp);

      const send = page.locator('button[aria-label="Send message"]:visible').first();
      await expect(send).toBeVisible({ timeout: 10_000 });

      await page.evaluate(() => {
        const els = document.querySelectorAll<HTMLTextAreaElement>('[data-role="prompt-input"]');
        for (const el of els) el.value = '';
      });
      // The face is still lit, because it was never reading the box.
      await expect(send).toBeVisible();
      await expect(input, 'the box was not blanked, so the state is not the reported one')
        .toHaveValue('');

      const box = await send.boundingBox();
      expect(box, 'the Send button never rendered').not.toBeNull();
      const x = Math.round(box!.x + box!.width / 2);
      const y = Math.round(box!.y + box!.height / 2);
      const cdp = await page.context().newCDPSession(page);
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y }] });
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });

      await expect
        .poll(sentCount, { timeout: 15_000, message: 'the press died on a box the send never needed' })
        .toBe('1');

      // And it says so, because a send carrying text the box is not showing is
      // exactly the state this whole round is about.
      await expect(page.locator('.toast', { hasText: 'Sent the saved draft' }))
        .toBeVisible({ timeout: 10_000 });
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  // The case both retired tests refused, and the one an emulator CAN drive: a
  // press that moves before it lifts. A thumb reaching a 29 px circle over the
  // keyboard rolls, and the visual viewport settles under a finger that did
  // not. Neither is a reason to throw the press away, and doing so silently is
  // how this reached a third report.
  //
  // It pins the composed behaviour rather than the helper alone. The tap gate
  // sees the same movement through `pointermove`, and must NOT veto here. If it
  // does, the dead button becomes a "Tap ignored" toast and still no message.
  test('a press that travels before lifting still sends', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    // Chromium injects real touch input over CDP. WebKit exposes no equivalent
    // on a mobile context, same as the tap case above.
    test.skip(browserName !== 'chromium', 'needs CDP touch injection');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const title = `E2E Travel ${suffix}`;
    const followUp = `finger rolled ${suffix}`;
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

      await page.evaluate((h: number) => {
        document.documentElement.style.setProperty('--app-height', `${h}px`);
      }, KEYBOARD_APP_HEIGHT_PX);

      const input = page.locator('[data-role="prompt-input"]:visible').first();
      await input.focus();
      await input.fill(followUp);

      const send = page.locator('button[aria-label="Send message"]:visible').first();
      await expect(send).toBeVisible({ timeout: 10_000 });
      const box = await send.boundingBox();
      expect(box, 'the Send button never rendered').not.toBeNull();

      const x = Math.round(box!.x + box!.width / 2);
      const y = Math.round(box!.y + box!.height / 2);
      const cdp = await page.context().newCDPSession(page);
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y }] });
      // Well past the 8 px both retired tests refused at, and past the tap
      // gate's own threshold, so this drives the composed decision.
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x, y: y + 20 }] });
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });

      await expect
        .poll(sentCount, { timeout: 15_000, message: 'a press that moved 20px was thrown away' })
        .toBe('1');
      await expect(input, 'the draft survived the send').toHaveValue('');

      // No "Tap ignored" toast either: the gate must have stayed on the click
      // path, where the press never arrived.
      await expect(page.locator('.toast', { hasText: 'Tap ignored' })).toHaveCount(0);

      await page.waitForTimeout(LATE_CLICK_GRACE_MS);
      expect(sentCount(), 'one press sent the message twice').toBe('1');
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  // Diff sits in the same row as Send and was reported dead in the same state,
  // alongside the answer Submit and the lone Cancel. It was click-only, and the
  // click is the path the keyboard makes unreliable. It is not destructive, so
  // it takes the touch path with none of the reasons Stop and Cancel decline
  // it. Same limit as the two cases above: an emulator cannot reproduce the
  // dropped click, so what this holds is that the touch path exists and runs.
  test('Diff opens on one touch tap with the keyboard up', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    test.skip(browserName !== 'chromium', 'needs CDP touch injection');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const cc = createCCThreadWithChange('E2E Diff Tap', suffix);

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("E2E Diff Tap ${suffix}")`).first().click();
      await ensureOnThreadPane(page);

      await page.evaluate((h: number) => {
        document.documentElement.style.setProperty('--app-height', `${h}px`);
      }, KEYBOARD_APP_HEIGHT_PX);

      // The reported state: the composer focused with a draft, so the keyboard
      // is up and `data-keyboard-active` is set.
      const input = page.locator('[data-role="prompt-input"]:visible').first();
      await input.focus();
      await input.fill(`typing while reviewing ${suffix}`);
      await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');

      const diff = page.locator('.prompt-actions-row button:has-text("Diff")').first();
      await expect(diff).toBeVisible({ timeout: 10_000 });
      const box = await diff.boundingBox();
      expect(box, 'the Diff button never rendered').not.toBeNull();

      const x = Math.round(box!.x + box!.width / 2);
      const y = Math.round(box!.y + box!.height / 2);
      const cdp = await page.context().newCDPSession(page);
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y }] });
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });

      // What the tap must produce: the Files view, revealed in the content
      // pane. Both are unconditional in `viewThreadCcDiff`, ahead of the fetch.
      // The rendered diff is NOT the signal here, since it needs the repo
      // registered, which this seeded thread's workspace is not.
      await expect(page.locator('.app-header'))
        .toHaveAttribute('data-mobile-view', 'content', { timeout: 15_000 });
      await expect(page.locator('.files-source-switcher').first()).toBeVisible({ timeout: 15_000 });

      // The touch path suppresses the synthetic click, so the shared blur on
      // `click` never runs. The action drops the keyboard itself instead, and
      // that is what this checks: a click would have done the same.
      await page.waitForTimeout(LATE_CLICK_GRACE_MS);
      await expect(page.locator('html')).not.toHaveAttribute('data-keyboard-active', '');
    } finally {
      cleanupCCThread(cc);
    }
  });
});
