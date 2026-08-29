import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, disarmFollowSeed } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * Pressing a turn control leaves it on the same pixel.
 *
 * `withScrollAnchor` (CreateThreadView) holds the control the reader pressed.
 * Every turn in the transcript changes height around it, and the correction
 * writes the scroll offset that movement asks for.
 *
 * It has to be measured in DOUBLES. The platform rounds `offsetTop` to a whole
 * pixel. A delta built from two of them is wrong by the difference of the two
 * roundings, and that difference FLIPS SIGN between the states. Reported
 * 2026-08-11 as "a slight jump up and down as i toggle the show last answer",
 * with the two clues that pin the mechanism: it happened whether the steps
 * control was on or off, and it did not happen on a thread too short to scroll
 * (with nowhere to scroll there is no offset to be wrong about).
 *
 * Sub-pixel in CSS terms is not sub-pixel on screen: measured at 0.86px on the
 * iOS engine, which is 2.6 device pixels at the phone's 3x scale, and it re-lands
 * every line of text on a different device-pixel row, which is why the reader
 * read it as the spacing changing rather than as a scroll.
 *
 * A BROWSER test and not a unit test, deliberately: the rounding is the
 * platform's, and jsdom has no layout to round.
 *
 * The bound is half a pixel because that is the floor. Layout is fractional and
 * a scroll offset is not, so some residual is unavoidable; what the fix removes
 * is the part that was ours. Half a pixel is the guarantee `Math.round` gives,
 * rather than a number read off a run.
 *
 * NON-INTEGER ROOT FONT SIZES are the point of the scales below. At a whole-pixel
 * root every rem-authored height is a whole number too, the two roundings cancel
 * and the bug is invisible; the shipped mobile default is 112.5%.
 */

/** How far the reader's content may move on a press. See the header: this is the
 *  guarantee of rounding one number, not an observed value. A hair of slack for
 *  float noise in the comparison itself. */
const MAX_DRIFT_PX = 0.51;

const TURNS = 12;

function q(o: unknown): string {
  return JSON.stringify(o).replace(/'/g, "''");
}

/** A transcript whose turns each carry prose, tool steps and more prose: the
 *  shape that makes `hidesEarlierProse` true, so the full-response control has
 *  something to reveal and every turn's height really does change. Turn sizes
 *  vary so the two states cannot share a fractional offset by construction. */
function seedThread(title: string): string {
  const threadId = randomUUID();
  const base = Date.now() - TURNS * 60_000;
  const rows: string[] = [];
  let seq = 0;
  const ev = (type: string, payload: unknown) => {
    const created = new Date(base + seq * 1000).toISOString();
    seq++;
    rows.push(
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ` +
      `('${randomUUID()}', '${type}', '${q(payload)}'::jsonb, '${created}', 'thread', '${threadId}', '${threadId}')`
    );
  };
  const para = (tag: string, n: number, t: number) => Array.from({ length: n }, (_, i) =>
    `${tag} paragraph ${i + 1} of turn ${t + 1}. ${'The quick brown fox jumps over the lazy dog. '.repeat(1 + ((i + t) % 3))}`
  ).join('\n\n');
  for (let t = 0; t < TURNS; t++) {
    ev('MessageReceived', { text: `Question ${t + 1}: ${'please do some work and explain it. '.repeat(1 + (t % 4))}`, channel: 'chat' });
    ev('TextStreamed', { text: `Turn ${t + 1} opening.\n\n${para('Opening', 2 + (t % 4), t)}` });
    ev('ToolCalled', { name: 'run_python', args: { code: `print(${t})` }, description: `Run python, turn ${t + 1}` });
    ev('ToolResult', { name: 'run_python', result: `output ${t}` });
    ev('TextStreamed', { text: `Turn ${t + 1} conclusion.\n\n${para('Conclusion', 1 + (t % 3), t)}` });
    ev('ResponseGenerated', { text: `Turn ${t + 1} conclusion.`, model: 'mock', channel: 'chat' });
  }
  const last = new Date(base + seq * 1000).toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', '${title}', 'chat', '${last}', ${TURNS}, false, true, 'idle', 'inbox', false, 0)`,
    ...rows,
  ].join(';\n'));
  return threadId;
}

function dropThread(threadId: string): void {
  psql([
    `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
    `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
  ].join(';\n'));
}

/** Press the full-response control on the first turn whose header is on screen.
 *  Reports how far THAT CONTROL moved, and how much the transcript's height
 *  changed, so the case can be shown to be non-vacuous.
 *
 *  The control is what the correction holds, so the control is where its
 *  rounding shows up. `null` when no header is fully on screen, which a turn
 *  taller than the pane produces at some parks.
 *
 *  The press is dispatched INSIDE the page rather than through Playwright's
 *  click, because Playwright scrolls a target into view first and that scroll
 *  would be indistinguishable from the drift being measured. */
async function pressAndMeasure(page: Page): Promise<{ drift: number; heightChange: number } | null> {
  return page.evaluate(async () => {
    const tc = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(el => el.getBoundingClientRect().height > 0);
    if (!tc) return null;
    const box = tc.getBoundingClientRect();

    let btn: HTMLElement | null = null;
    for (const ex of Array.from(tc.querySelectorAll<HTMLElement>('.chat-exchange'))) {
      const h = ex.querySelector<HTMLElement>('.response-header');
      if (!h) continue;
      const r = h.getBoundingClientRect();
      if (r.top > box.top + 4 && r.bottom < box.bottom - 4) {
        btn = ex.querySelector<HTMLElement>('[data-role="toggle-details"]');
        break;
      }
    }
    if (!btn) return null;

    const before = btn.getBoundingClientRect().top;
    const heightBefore = tc.scrollHeight;
    btn.click();
    // Past the correction, its next-frame re-check, and any late settle.
    await new Promise(r => setTimeout(r, 600));
    if (!btn.isConnected) return null;
    return {
      drift: btn.getBoundingClientRect().top - before,
      heightChange: tc.scrollHeight - heightBefore,
    };
  });
}

/** Dismiss any toast before reaching into the drawer.
 *
 *  The toast container sits over the bottom of the pane, and at a phone width
 *  the drawer's rows are under it, so a toast the workspace happens to be
 *  showing (an engine-version notice, a change landing) intercepts the click
 *  that opens the thread and the spec times out with nothing to say about the
 *  anchor. Desktop is wide enough to miss it, which is exactly why this is worth
 *  doing rather than leaving to luck. Dismissing rather than force-clicking: a
 *  real finger cannot reach through a toast either, and `force` would hide a
 *  genuine overlap regression. */
async function clearToasts(page: Page): Promise<void> {
  await page.evaluate(() => {
    document.querySelectorAll<HTMLButtonElement>('.toast .toast-close').forEach(b => b.click());
  });
  await page.locator('.toast-container .toast').first().waitFor({ state: 'detached', timeout: 5_000 }).catch(() => {
    // Not every toast is dismissable. The click below retries for 15s anyway,
    // and an auto-expiring toast clears well inside that.
  });
}

async function park(page: Page, frac: number): Promise<void> {
  await page.locator('.thread-content.visible:visible').first().evaluate((el, f) => {
    el.scrollTop = Math.round((el.scrollHeight - el.clientHeight) * (f as number));
  }, frac);
  await page.waitForTimeout(300);
}

test.describe('A turn control holds itself still', () => {
  test("the full-response toggle moves the control by under half a pixel, at a fractional root font size", async ({ page }) => {
    await assertHealthy(page);
    const threadId = seedThread('Turn control anchor');
    try {
      // This spec is about a reader who has PARKED somewhere and pressed a
      // control. The follow seed ships armed, so without this the thread opens
      // riding the live edge. `park()` writes `scrollTop` directly rather than
      // as a gesture, and a direct write is not what disarms a ride. The
      // transcript therefore hauls the reader back to the end before the press
      // is made. What gets measured is that haul, hundreds of pixels of it,
      // rather than the sub-pixel drift under test.
      await disarmFollowSeed(page);
      await navigateToApp(page);
      await openThreadDrawer(page);
      await clearToasts(page);
      await page.locator('.thread-row:has-text("Turn control anchor")').first().click();
      await ensureOnThreadPane(page);

      const tc = page.locator('.thread-content.visible:visible').first();
      await expect
        .poll(() => tc.evaluate(el => el.querySelectorAll('.chat-exchange').length), { timeout: 25_000 })
        .toBe(TURNS);
      // Not vacuous: with nothing to scroll there is no offset to be wrong about,
      // which is exactly the case the reader reported as fine.
      await expect
        .poll(() => tc.evaluate(el => el.scrollHeight - el.clientHeight), { timeout: 10_000 })
        .toBeGreaterThan(500);

      let measured = 0;
      for (const scale of ['105%', '112.5%']) {
        await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', s), scale);
        await page.waitForTimeout(500);
        const rootPx = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize));

        for (let press = 1; press <= 4; press++) {
          await park(page, 0.3 + (press % 3) * 0.15);
          const m = await pressAndMeasure(page);
          if (!m) continue;
          measured++;
          // Non-vacuous the other way too: the press must really have changed
          // the transcript's height, or holding the reader still is free.
          expect(
            Math.abs(m.heightChange),
            `scale ${scale} press ${press}: the toggle changed no height, so the case proves nothing`,
          ).toBeGreaterThan(100);
          expect(
            Math.abs(m.drift),
            `scale ${scale} (root ${rootPx}px) press ${press}: the control moved ${m.drift}px`,
          ).toBeLessThanOrEqual(MAX_DRIFT_PX);
        }
      }
      expect(measured, 'no press was measurable, so the spec asserted nothing').toBeGreaterThanOrEqual(4);
    } finally {
      dropThread(threadId);
    }
  });
});
