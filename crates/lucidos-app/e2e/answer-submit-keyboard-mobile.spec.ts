import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, isMobileViewport } from './helpers';
import { psql, createCCThreadWithChange, cleanupCCThread } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * The composer's Submit answers a question while the mobile keyboard is up.
 *
 * A question card parks the thread on `waiting_for_user_answer`, and typing a
 * custom answer swaps the round send morph for a lone green Submit. Reported
 * from an iOS PWA: with the keyboard up, that Submit did nothing, tap after tap,
 * while the transcript still scrolled and the keyboard stayed on screen. The
 * round send morph on an ordinary thread was fine.
 *
 * This builds that row and pins the LAYOUT half. Submit is hittable at its
 * centre and edges. Nothing overlays it. The row never rebuilds it under the
 * finger, and an injected touch tap answers.
 *
 * It never reproduced the failure, and that is the finding. The cause was the
 * scroll-vs-tap gate reading page-viewport coordinates, which no emulator
 * without a keyboard can trip. The gate is covered by
 * `src/utils/tapGesture.test.ts` and `prompt-cancel-tap-gate.test.ts`.
 */

const OPTIONS = [
  { id: 'approve', label: 'Approve it', description: 'Go ahead with the plan as written.' },
  { id: 'keep', label: 'Keep the preference key', description: 'Leave the key alone.' },
];

/** Visual-viewport height an open iOS keyboard leaves on this device, which is
 *  what MobileSwipeContainer writes to `--app-height`. */
const KEYBOARD_APP_HEIGHT_PX = 490;

test.describe('Answer Submit with the mobile keyboard up', () => {
  // iPhone 15 Pro portrait points, the device the report came from.
  test.use({ viewport: { width: 393, height: 852 } });

  test('is hittable where it is painted, and a touch tap runs it', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const toolUseId = `tu-answer-${suffix}`;
    const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Answer', suffix);
    const now = new Date().toISOString();
    const question = JSON.stringify({
      tool_use_id: toolUseId,
      cc_session_id: '',
      channel: 'claude_code',
      multi_select: false,
      question: `Approve it, or keep the preference key? ${suffix}`,
      options: OPTIONS,
    }).replace(/'/g, "''");

    psql([
      `UPDATE thread_summaries SET status = 'waiting_for_user_answer' WHERE thread_id = '${threadId}'`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${question}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("E2E Answer ${suffix}")`).first().click();
      await ensureOnThreadPane(page);
      await expect(page.locator(`.question-body[data-tool-use-id="${toolUseId}"]`).first())
        .toBeVisible({ timeout: 15_000 });

      // The reported conditions: the user's ui scale, and the app shell shrunk
      // to what an open keyboard leaves of the visual viewport.
      await page.evaluate((h: number) => {
        document.documentElement.style.setProperty('--user-ui-scale', '112.5%');
        document.documentElement.style.setProperty('--app-height', `${h}px`);
      }, KEYBOARD_APP_HEIGHT_PX);

      // Typing a custom answer is what swaps the lone red Cancel for Submit.
      const input = page.locator('[data-role="prompt-input"]:visible').first();
      await input.focus();
      await input.fill('Its good, but I would like to see the plan visualized and scrutinise it more.');
      await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');

      const submit = page.locator('button[aria-label="Submit answer"]:visible').first();
      await expect(submit).toBeVisible({ timeout: 10_000 });
      await expect(page.locator('.prompt-actions-row button:has-text("Diff")').first())
        .toBeVisible({ timeout: 10_000 });

      // Is the row settled, or is the fit check flipping it between its two
      // layouts? A reparented button is destroyed and rebuilt, and a touch that
      // began on the old node produces no click at all.
      const churn = await page.evaluate(async () => {
        const find = () => Array.from(document.querySelectorAll<HTMLElement>('button[aria-label="Submit answer"]'))
          .find(b => b.getBoundingClientRect().width > 0) ?? null;
        const stackedReadings = new Set<boolean>();
        let replaced = 0;
        let previous = find();
        for (let i = 0; i < 40; i++) {
          await new Promise(r => setTimeout(r, 50));
          const current = find();
          if (current !== previous) replaced++;
          previous = current;
          stackedReadings.add(!!document.querySelector('.prompt-actions-right.is-stacked'));
        }
        return { replaced, stackedReadings: Array.from(stackedReadings) };
      });

      const probe = await page.evaluate(() => {
        const btn = Array.from(document.querySelectorAll<HTMLElement>('button[aria-label="Submit answer"]'))
          .find(b => b.getBoundingClientRect().width > 0);
        if (!btn) return null;
        const r = btn.getBoundingClientRect();
        const cx = r.left + r.width / 2;
        const cy = r.top + r.height / 2;
        const describe = (el: Element | null) => {
          if (!el) return 'null';
          const tag = el.tagName.toLowerCase();
          const cls = (el.getAttribute('class') || '').trim();
          const label = el.getAttribute('aria-label') || '';
          return `${tag}${cls ? '.' + cls.split(/\s+/).join('.') : ''}${label ? `[${label}]` : ''}`;
        };
        const at = (x: number, y: number) => describe(document.elementFromPoint(x, y));
        const row = btn.closest('.prompt-actions-row') as HTMLElement;
        const rowRect = row.getBoundingClientRect();
        const rowCs = getComputedStyle(row);
        const shell = document.querySelector('.app-shell') as HTMLElement;
        return {
          rect: { top: r.top, bottom: r.bottom, left: r.left, right: r.right },
          pointerEvents: getComputedStyle(btn).pointerEvents,
          centre: at(cx, cy),
          topEdge: at(cx, r.top + 2),
          bottomEdge: at(cx, r.bottom - 2),
          leftEdge: at(r.left + 2, cy),
          rightEdge: at(r.right - 2, cy),
          stack: Array.from(document.elementsFromPoint(cx, cy)).slice(0, 6).map(describe),
          stacked: !!document.querySelector('.prompt-actions-right.is-stacked'),
          rowContent: {
            left: rowRect.left + (parseFloat(rowCs.paddingLeft) || 0),
            right: rowRect.right - (parseFloat(rowCs.paddingRight) || 0),
          },
          items: Array.from(row.querySelectorAll<HTMLElement>('[data-row-item]')).map((el) => {
            const b = el.getBoundingClientRect();
            return { d: el.getAttribute('aria-label') ?? el.textContent?.trim() ?? el.className, l: b.left, r: b.right };
          }),
          shellBottom: shell.getBoundingClientRect().bottom,
          innerHeight: window.innerHeight,
          appHeight: getComputedStyle(document.documentElement).getPropertyValue('--app-height'),
        };
      });

      expect(probe, 'the Submit button never rendered').not.toBeNull();
      expect(churn.replaced, 'the Submit button is being rebuilt under the finger').toBe(0);
      expect(probe!.pointerEvents, 'Submit is inert while the keyboard is up').not.toBe('none');
      expect(probe!.centre, 'something else answers the pointer at Submit\'s centre')
        .toContain('Submit answer');
      expect(probe!.bottomEdge, 'something covers Submit\'s bottom edge')
        .toContain('Submit answer');
      expect(probe!.rect.bottom, 'Submit sits below the app shell, behind the keyboard')
        .toBeLessThanOrEqual(probe!.shellBottom + 0.5);

      // Chromium injects real touch input over CDP, so the tap itself is
      // testable there. WebKit exposes no equivalent on a mobile context.
      if (browserName === 'chromium') {
        const r = probe!.rect;
        const x = Math.round((r.left + r.right) / 2);
        const y = Math.round((r.top + r.bottom) / 2);
        const cdp = await page.context().newCDPSession(page);
        await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y }] });
        await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });

        await expect
          .poll(
            () => psql(`SELECT count(*) FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered'`),
            { timeout: 15_000, message: 'a touch tap on Submit never produced an answer' },
          )
          .toContain('1');
      }
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
