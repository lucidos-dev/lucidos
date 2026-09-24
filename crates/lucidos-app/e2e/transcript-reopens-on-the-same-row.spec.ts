import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy, disarmFollowSeed } from './helpers';
import { psql } from './db-helpers';

/** A long coding-agent thread reopens on the ROW the reader was on.
 *
 *  Reported: "its not remembering the position correctly" and "sometimes it
 *  starts at the bottom". One coding-agent turn holds hundreds of steps, and
 *  the render window draws a turn's tail first. A position that named the turn
 *  measured from the clamped top, and a window reseeded on reopen shrank under
 *  a restore that had already landed. The position now names the step row.
 *  Plan: docs/plans/2026-09-23-the-reading-position-and-the-thumb-hold-still.md */

/** Steps per turn. Turn 3 is far larger than the window's row budget, and the
 *  whole thread runs past the first page of 400 events. */
const STEPS = [30, 300, 250, 300, 40];

function seed(title: string, steps: number[]): string {
  const threadId = randomUUID();
  const base = Date.now() - 3_600_000;
  let clock = 0;
  const at = () => new Date(base + (clock++) * 10).toISOString();
  const row = (id: string, type: string, payload: string) =>
    `('${id}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;
  const rows: string[] = [];
  steps.forEach((n, t) => {
    const messageId = randomUUID();
    rows.push(row(messageId, 'MessageReceived', `{"text":"turn ${t}","mode":"human","channel":"claude_code"}`));
    for (let s = 0; s < n; s++) {
      const useId = `row-${threadId.slice(0, 8)}-${t}-${s}`;
      rows.push(row(randomUUID(), 'CodingAgentToolCalled',
        `{"name":"Bash","args":{"command":"echo t${t}s${s}"},"description":"Run t${t}s${s}",` +
        `"channel":"claude_code","tool_use_id":"${useId}","coding_agent":"claude-code","request_event_id":"${messageId}"}`));
      rows.push(row(randomUUID(), 'CodingAgentToolResult',
        `{"name":"","result":"ok","channel":"claude_code","tool_use_id":"${useId}",` +
        `"coding_agent":"claude-code","request_event_id":"${messageId}"}`));
    }
    rows.push(row(randomUUID(), 'ResponseGenerated',
      `{"text":"Finished turn ${t}.","images":[],"request_event_id":"${messageId}"}`));
  });
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', '${title}', 'claude_code', now(), ${steps.length}, false, true, 'idle', 'active', 'active', true, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return threadId;
}

/** The step row resting on the transcript's top line and its offset, the same
 *  rule `readRowAnchor` uses, restated so the spec asserts the reader's view. */
async function rowAtTop(page: Page): Promise<{ label: string; relTop: number } | null> {
  return page.locator('.thread-content').first().evaluate((el) => {
    const top = el.getBoundingClientRect().top;
    let best: { label: string; relTop: number } | null = null;
    for (const row of Array.from(el.querySelectorAll<HTMLElement>('[data-row-event]'))) {
      const rect = row.getBoundingClientRect();
      if (rect.height <= 0) continue;
      const relTop = Math.round(rect.top - top);
      if (relTop > 0) break;
      best = { label: (row.textContent ?? '').replace(/\s+/g, ' ').trim().slice(0, 24), relTop };
    }
    return best;
  });
}

/** Scroll up by `px` the way a reader does: a wheel event, then the move. */
async function scrollUp(page: Page, px: number): Promise<void> {
  await page.locator('.thread-content').first().evaluate((el, by) => {
    el.dispatchEvent(new WheelEvent('wheel', { deltaY: -by, bubbles: true }));
    el.scrollTop = Math.max(0, el.scrollTop - by);
  }, px);
  await page.waitForTimeout(200);
}

test.describe('A long coding-agent thread reopens on the same row', () => {
  let threadA = '';
  let threadB = '';

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    threadA = seed('E2E same row long', STEPS);
    threadB = seed('E2E same row short', [3, 3, 3]);
  });

  test.afterEach(() => {
    psql(`DELETE FROM events WHERE thread_id IN ('${threadA}','${threadB}'); DELETE FROM thread_summaries WHERE thread_id IN ('${threadA}','${threadB}')`);
  });

  test('after a switch away and back, and after a reload', async ({ page }) => {
    test.setTimeout(180_000);
    await page.addInitScript((tid: string) => {
      if (!sessionStorage.getItem('seeded')) {
        localStorage.setItem('lucidos-focused-thread', tid);
        sessionStorage.setItem('seeded', '1');
      }
    }, threadA);
    await disarmFollowSeed(page);
    await navigateToApp(page);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // Walk up into the early rows of turn 3, the big turn the window draws
    // with its head clamped off on every fresh open.
    for (let i = 0; i < 80; i++) {
      const at = await rowAtTop(page);
      if (at && /Run t3s([0-9]|[1-5][0-9])\b/.test(at.label)) break;
      await scrollUp(page, 400);
    }
    await page.waitForTimeout(600);
    const parked = await rowAtTop(page);
    expect(parked?.label, 'parked deep inside the big turn').toMatch(/Run t3s\d+/);

    // Away and back, the way a reader switches threads.
    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadB);
    await expect(transcript.getByText('Run t0s0').first()).toBeVisible();
    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadA);
    await expect.poll(async () => (await rowAtTop(page))?.label, { timeout: 10_000 }).toBe(parked!.label);
    const back = await rowAtTop(page);
    expect(Math.abs(back!.relTop - parked!.relTop)).toBeLessThanOrEqual(2);
    // And it stays there: nothing shrinks the window under a restore that landed.
    await page.waitForTimeout(1500);
    expect((await rowAtTop(page))?.label).toBe(parked!.label);

    // A reload loads only the newest page, so the row is found by the chase.
    await page.reload();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();
    await expect.poll(async () => (await rowAtTop(page))?.label, { timeout: 15_000 }).toBe(parked!.label);
    const reloaded = await rowAtTop(page);
    expect(Math.abs(reloaded!.relTop - parked!.relTop)).toBeLessThanOrEqual(2);
    await page.waitForTimeout(1500);
    expect((await rowAtTop(page))?.label).toBe(parked!.label);
    const stillScrollable = await transcript.evaluate((el) => el.scrollTop < el.scrollHeight - el.clientHeight - 1);
    expect(stillScrollable, 'never parked at the bottom').toBe(true);
  });

  /** Reported: "this thread consistently opens in one position and adjusts to
   *  new after a sec or so". A reader who scrolled up into the first turn
   *  holds a window that draws the whole thread, and a reopen must keep it.
   *  Re-seeded, the thread opens on the newest turns while the restore walks
   *  back up, and jumps when it arrives. */
  test('a reader who scrolled to the first turn reopens there without a jump', async ({ page }) => {
    test.setTimeout(180_000);
    await page.addInitScript((tid: string) => {
      if (!sessionStorage.getItem('seeded')) {
        localStorage.setItem('lucidos-focused-thread', tid);
        sessionStorage.setItem('seeded', '1');
      }
    }, threadA);
    await disarmFollowSeed(page);
    await navigateToApp(page);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // Scroll up the way a reader does until the transcript rests at the true
    // top: every page folded in and the first turn drawn from its first row.
    const atTrueTop = () => transcript.evaluate((el) => el.scrollTop === 0
      && (el.querySelector('[data-row-event]')?.textContent ?? '').includes('Run t0s0'));
    for (let i = 0; i < 120; i++) {
      if (await atTrueTop()) {
        await page.waitForTimeout(800);
        if (await atTrueTop()) break;
      }
      await scrollUp(page, 800);
    }
    expect(await atTrueTop(), 'rests at the top of the first turn').toBe(true);
    // Park a few rows down inside that first turn.
    await transcript.evaluate((el) => {
      el.dispatchEvent(new WheelEvent('wheel', { deltaY: 300, bubbles: true }));
      el.scrollTop += 300;
    });
    await page.waitForTimeout(600);
    const parked = await rowAtTop(page);
    expect(parked?.label, 'parked inside the first turn').toMatch(/Run t0s\d+/);

    // Away to the short thread. Its three turns are what prove the switch
    // landed, since A's own DOM also holds every row B has.
    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadB);
    await expect(transcript.locator('.chat-exchange')).toHaveCount(3);

    // Watch every frame of the return. Frames before the transcript has any
    // row on its top line are fine; a DIFFERENT row there is the jump.
    const seen = await page.evaluate(async (tid) => {
      const labels: string[] = [];
      location.hash = `#thread=${tid}`;
      const start = performance.now();
      while (performance.now() - start < 2500) {
        await new Promise(r => requestAnimationFrame(r));
        const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
          .find(e => e.getBoundingClientRect().height > 0);
        if (!el) continue;
        const top = el.getBoundingClientRect().top;
        let label: string | null = null;
        for (const row of Array.from(el.querySelectorAll<HTMLElement>('[data-row-event]'))) {
          const rect = row.getBoundingClientRect();
          if (rect.height <= 0) continue;
          if (rect.top - top > 0) break;
          label = (row.textContent ?? '').replace(/\s+/g, ' ').trim().slice(0, 24);
        }
        if (label !== null && labels[labels.length - 1] !== label) labels.push(label);
      }
      return labels;
    }, threadA);
    expect(seen, 'only the parked row ever sits on the top line').toEqual([parked!.label]);
  });
});
