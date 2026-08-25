import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, disableMobileHeaderSticky, disarmFollowSeed, enableMobileHeaderSticky, ensureOnThreadPane, navigateToApp } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * **A turn control holds the reader wherever they are parked.**
 *
 * Its siblings `steps-toggle-holds-the-reader.spec.ts` and
 * `turn-control-holds-the-reader-still.spec.ts` press from hand-picked
 * positions. Each is a shape somebody reported, and each passed while the
 * control still moved the reader from positions nobody had tried. This one
 * SWEEPS the transcript: park at 23 offsets, press at each, and report every
 * one that drifted. See `CONTROLS` for which control it sweeps, and why.
 *
 * The sweep found five causes, and no hand-picked park would have found them.
 */

/* The five:
 *
 * - the reader resting in a turn's trailing chrome, where the anchor scan read
 *   only the turn their edge was in and answered "no line";
 * - the reader resting in the margin between two rows, where the seam was
 *   refused;
 * - a seam's own landing pixel, which left the row above straddling the sliver
 *   the next press scans by;
 * - the mobile chrome sliding, because the hide-on-scroll header read the
 *   correction's own write as the reader scrolling;
 * - a clamp debt measured by reading `scrollTop` back from that write.
 */

/* What is asserted is the contract, not a number: the row at the reader's first
 * readable line stays there. Where the reveal TOOK that row, the run it was in
 * collapses to a seam and the reader rests on it.
 *
 * Both readings are measured from the first line the reader can READ, which on
 * mobile is below the sticky thread title. So chrome sliding over the
 * transcript fails as loudly as content moving under it. */

const TURNS = 6;
const CHUNKS = 10;
const STEPS_PER_CHUNK = 4;
/** Parks across the scrollable range, one press each. */
const PARKS = 24;
/** How far the reader may move, in CSS px. A correction is written to a whole
 *  pixel and a sub-pixel row height can round the other way. */
const TOLERANCE_PX = 4;

function q(o: unknown): string {
  return JSON.stringify(o).replace(/'/g, "''");
}

/** Turns of prose interleaved with runs of tool steps. Every row carries its
 *  own mark, so it is findable again after the DOM has changed under it. The
 *  prose wraps and the runs are long, which is what makes a press take a large
 *  bite out of the content above the reader. */
function seedThread(title: string): string {
  const threadId = randomUUID();
  const base = Date.now() - 600_000;
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
  for (let t = 0; t < TURNS; t++) {
    ev('MessageReceived', { text: `Question ${t}, please look it up.`, channel: 'chat' });
    for (let c = 0; c < CHUNKS; c++) {
      const body = 'The quick brown fox jumps over the lazy dog and keeps running. '.repeat(4);
      ev('TextStreamed', { text: `P-${t}-${c} ${body}` });
      for (let s = 0; s < STEPS_PER_CHUNK; s++) {
        ev('ToolCalled', {
          name: 'run_bash',
          args: { command: `echo S-${t}-${c}-${s}` },
          description: `S-${t}-${c}-${s} run a shell command with a fairly long description that may wrap`,
        });
        ev('ToolResult', { name: 'run_bash', result: `out ${t}.${c}.${s}` });
      }
    }
    const answer = `Answer ${t}.`;
    ev('TextStreamed', { text: answer });
    ev('ResponseGenerated', { text: answer, model: 'mock', channel: 'chat' });
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

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  // The seed ships ARMED, and a rider is carried back to the live edge by any
  // scroll that is not their own gesture. This spec is about the reader who is
  // READING, so it starts them disarmed.
  await disarmFollowSeed(page);
  // Hide-on-scroll LIVE rather than inherited: see `disableMobileHeaderSticky`.
  await disableMobileHeaderSticky(page);
  await navigateToApp(page);
  await ensureOnThreadPane(page);
  await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 30_000 });
  await expect(page.locator('[data-role="inline-step"]:visible').first()).toBeVisible({ timeout: 30_000 });
}

interface Probe {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  /** How far the first READABLE line sits below the container's own top, i.e.
   *  what the mobile sticky title covers. Reported so a failure says whether
   *  the content moved or the chrome did. */
  inset: number;
  rows: Array<{ id: string; top: number; bottom: number }>;
}

/** Every response row, keyed by its own text, measured against the first line
 *  the reader can read. The edge rule mirrors `readerTopEdge`. */
async function probe(page: Page): Promise<Probe> {
  return await page.evaluate(() => {
    const el = document.querySelector('.thread-content') as HTMLElement;
    const containerTop = el.getBoundingClientRect().top;
    let edge = containerTop;
    for (const child of Array.from(el.children) as HTMLElement[]) {
      if (!child.matches('[data-scroller-pinned]')) continue;
      const r = child.getBoundingClientRect();
      if (r.height > 0 && r.bottom > edge) edge = r.bottom;
    }
    const rows = Array.from(el.querySelectorAll<HTMLElement>('.response-content > *')).map(r => {
      const b = r.getBoundingClientRect();
      return { id: (r.textContent || '').trim().slice(0, 40), top: b.top - edge, bottom: b.bottom - edge };
    });
    return {
      scrollTop: el.scrollTop,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
      inset: edge - containerTop,
      rows,
    };
  });
}

/** Press `control` on the turn holding `rowId`.
 *
 *  Dispatched rather than driven through the pointer. Playwright scrolls a
 *  control into view first. A tall turn's header is off the top of the screen,
 *  so the drive itself would move the reader. */
async function pressOnTurnHolding(page: Page, control: string, rowId: string): Promise<boolean> {
  return await page.evaluate(({ role, needle }: { role: string; needle: string }) => {
    const el = document.querySelector('.thread-content') as HTMLElement;
    const row = Array.from(el.querySelectorAll<HTMLElement>('.response-content > *'))
      .find(r => (r.textContent || '').trim().slice(0, 40) === needle);
    const btn = row?.closest('.chat-exchange')?.querySelector<HTMLElement>(`[data-role="${role}"]`);
    if (!btn) return false;
    btn.click();
    return true;
  }, { role: control, needle: rowId });
}

/** How far the press moved the reader, and what was measured to say so.
 *
 *  Their own line if it survived. Otherwise the seam its run collapsed to: the
 *  nearest surviving row above resting its bottom on the edge, or the nearest
 *  below resting its top there. Either is a legitimate seam, so whichever lands
 *  is taken and only a press landing on neither counts as a drift. */
function drift(before: Probe, after: Probe, i: number): { off: number; what: string } | null {
  const line = before.rows[i];
  const survivor = after.rows.find(r => r.id === line.id);
  if (survivor) return { off: survivor.top - line.top, what: `line "${line.id}" holds its place` };
  const candidates: Array<{ off: number; what: string }> = [];
  for (let j = i - 1; j >= 0; j--) {
    const s = after.rows.find(r => r.id === before.rows[j].id);
    if (s) { candidates.push({ off: s.bottom, what: `seam under "${s.id}"` }); break; }
  }
  for (let j = i + 1; j < before.rows.length; j++) {
    const s = after.rows.find(r => r.id === before.rows[j].id);
    if (s) { candidates.push({ off: s.top, what: `seam over "${s.id}"` }); break; }
  }
  if (candidates.length === 0) return null;
  return candidates.reduce((best, c) => (Math.abs(c.off) < Math.abs(best.off) ? c : best));
}

/** The STEP LOG, and deliberately not the full-response control beside it.
 *
 *  Both run through `withScrollAnchor`, so both take these fixes. What differs
 *  is what a press LEAVES. The step log takes a contiguous run and the rows
 *  around it survive, which is what makes a seam nameable and this sweep's
 *  assertion meaningful.
 *
 *  The full response keeps only each turn's LAST text block. One press can
 *  therefore take everything on screen and most of the transcript with it.
 *  There is no run and no seam, so the sweep can say nothing true about where
 *  the reader belongs. It has its own spec:
 *  `turn-control-holds-the-reader-still.spec.ts`. */
const CONTROLS = [
  { role: 'toggle-steps', name: 'the steps' },
] as const;

test.describe('a turn control holds every reading position', () => {
  // `openThread` turns the global header pin off. It is global and the e2e
  // database resets only between projects, so put it back. See
  // `disableMobileHeaderSticky`.
  test.afterEach(async ({ page }) => {
    await enableMobileHeaderSticky(page);
  });

  for (const control of CONTROLS) {
    for (const direction of ['hide', 'show'] as const) {
      test(`${direction} ${control.name}, swept across the transcript`, async ({ page }) => {
      await assertHealthy(page);
      const threadId = seedThread(`Sweep ${control.role} ${direction}`);
      const failures: string[] = [];
      try {
        await openThread(page, threadId);
        // Start from the opposite state, so every press in the sweep is the
        // direction under test. Any turn's control does it, the setting being
        // transcript-wide.
        if (direction === 'show') {
          const anyToggle = page.locator(`[data-role="${control.role}"]`).first();
          await anyToggle.dispatchEvent('click');
          await expect(anyToggle).toHaveAttribute('aria-pressed', 'false');
          await page.waitForTimeout(300);
        }

        const opened = await probe(page);
        const span = opened.scrollHeight - opened.clientHeight;
        expect(span, 'the seeded thread must be several screens tall').toBeGreaterThan(4000);

        for (let k = 1; k < PARKS; k++) {
          const park = Math.round((span * k) / PARKS);
          await page.evaluate((top: number) => {
            const el = document.querySelector('.thread-content') as HTMLElement;
            el.scrollTop = top;
          }, park);
          await page.waitForTimeout(150);

          const before = await probe(page);
          const i = before.rows.findIndex(r => r.bottom > 1);
          if (i < 0) continue;
          if (!await pressOnTurnHolding(page, control.role, before.rows[i].id)) continue;
          await page.waitForTimeout(350);

          const after = await probe(page);
          const moved = drift(before, after, i);
          if (moved && Math.abs(moved.off) > TOLERANCE_PX) {
            failures.push(
              `park ${park}: ${moved.what}, off by ${moved.off.toFixed(1)}px`
              + ` (readable edge ${before.inset.toFixed(1)} -> ${after.inset.toFixed(1)},`
              + ` scrollTop ${before.scrollTop} -> ${after.scrollTop})`,
            );
          }

          // Put the toggle back, so the next park presses the same direction.
          const back = direction === 'hide' ? 'true' : 'false';
          await page.evaluate((role: string) => {
            document.querySelector<HTMLElement>(`.thread-content [data-role="${role}"]`)?.click();
          }, control.role);
          await page.waitForTimeout(250);
          await expect(page.locator(`[data-role="${control.role}"]`).first())
            .toHaveAttribute('aria-pressed', back);
        }
        expect(failures.join('\n'), `${failures.length} of ${PARKS - 1} parks moved the reader`).toBe('');
      } finally {
        dropThread(threadId);
      }
      });
    }
  }
});
