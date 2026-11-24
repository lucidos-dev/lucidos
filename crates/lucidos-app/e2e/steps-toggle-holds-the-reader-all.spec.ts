import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, enableMobileDynamicBars, disarmFollowSeed, disableMobileDynamicBars, ensureOnThreadPane, navigateToApp, renderWholeTranscript, waitForScrollSettled } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';


const TURNS = 8;
const CHUNKS_PER_TURN = 14;
const STEPS_PER_CHUNK = 1;
const STEPS_PER_RUN = 40;
const DRIFT_TOLERANCE_PX = 4;

const mark = (turn: number, chunk: number) => `MARK-${turn}-${chunk}`;

function q(o: unknown): string {
  return JSON.stringify(o).replace(/'/g, "''");
}

function seedThread(
  title: string,
  { stepsPerChunk = STEPS_PER_CHUNK, chunks = CHUNKS_PER_TURN, proseLines = 1, turns = TURNS } = {},
): string {
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
  for (let t = 0; t < turns; t++) {
    ev('MessageReceived', { text: `Question number ${t}.`, channel: 'chat' });
    for (let c = 0; c < chunks; c++) {
      if (proseLines > 0) {
        const body = 'The quick brown fox jumps over the lazy dog. '.repeat(6 * proseLines);
        ev('TextStreamed', { text: `${mark(t, c)} part ${c}. ${body}` });
      }
      for (let s = 0; s < stepsPerChunk; s++) {
        ev('ToolCalled', {
          name: 'web_search',
          args: { query: `turn ${t} step ${c}.${s}` },
          description: `Search the web for turn ${t} step ${c}.${s}`,
        });
        ev('ToolResult', { name: 'web_search', result: `result ${t}.${c}.${s}` });
      }
    }
    const answer = `Answer to question ${t}.`;
    ev('TextStreamed', { text: answer });
    ev('ResponseGenerated', { text: answer, model: 'mock', channel: 'chat' });
  }
  const last = new Date(base + seq * 1000).toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', '${title}', 'chat', '${last}', ${turns}, false, true, 'idle', 'inbox', false, 0)`,
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

async function openThread(page: Page, threadId: string, { disarm = true, renderAll = true } = {}): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  if (disarm) await disarmFollowSeed(page);
  await enableMobileDynamicBars(page);
  await navigateToApp(page);
  await ensureOnThreadPane(page);
  await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 30_000 });
  await expect(page.locator('[data-role="inline-step"]:visible').first()).toBeVisible({ timeout: 30_000 });
  if (renderAll) await renderWholeTranscript(page);
}

async function turnCarrying(page: Page, text: string): Promise<string> {
  return await page.evaluate((needle: string) => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    const row = Array.from(el.querySelectorAll<HTMLElement>('.response-content > *'))
      .find(r => (r.textContent ?? '').includes(needle));
    if (!row) throw new Error(`no row carrying ${needle}`);
    const turn = row.closest('.chat-exchange') as HTMLElement | null;
    const seq = turn?.getAttribute('data-user-seq') ?? '';
    if (!seq) throw new Error(`the row carrying ${needle} is in no turn`);
    return seq;
  }, text);
}

function controlSelector(seq: string, role: string): string {
  return `.chat-exchange[data-user-seq="${seq}"] .response-header .turn-controls [data-role="${role}"]`;
}

function controlOn(page: Page, seq: string, role = 'toggle-steps') {
  return page.locator(controlSelector(seq, role)).first();
}


interface Geometry {
  top: number;
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

async function geometry(page: Page, seq: string, role = 'toggle-steps'): Promise<Geometry> {
  return await page.evaluate(({ sel, r, s }: { sel: string; r: string; s: string }) => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    const btn = el.querySelector<HTMLElement>(sel);
    if (!btn) throw new Error(`no ${r} control on turn ${s}`);
    return {
      top: btn.getBoundingClientRect().top - el.getBoundingClientRect().top,
      scrollTop: el.scrollTop,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
    };
  }, { sel: controlSelector(seq, role), r: role, s: seq });
}

async function parkControlMidScreen(page: Page, seq: string): Promise<void> {
  for (let round = 0; round < 5; round++) {
    const g = await geometry(page, seq);
    const want = g.clientHeight / 2;
    if (Math.abs(g.top - want) <= 1) return;
    await page.evaluate((d: number) => {
      const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
        .find(c => c.getBoundingClientRect().height > 0)!;
      el.scrollTop += d;
    }, g.top - want);
    await waitForScrollSettled(page);
  }
}

async function pressControlOn(page: Page, seq: string, role: string, expected: 'true' | 'false'): Promise<void> {
  const toggle = controlOn(page, seq, role);
  await toggle.dispatchEvent('click');
  await expect(toggle).toHaveAttribute('aria-pressed', expected);
  await waitForScrollSettled(page);
}

async function pressStepsOn(page: Page, seq: string, expected: 'true' | 'false'): Promise<void> {
  await pressControlOn(page, seq, 'toggle-steps', expected);
}

const LIVE_EDGE_GAP_PX = 2;

async function parkAtLiveEdge(page: Page): Promise<void> {
  await expect.poll(async () => {
    await page.evaluate(() => {
      const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
        .find(c => c.getBoundingClientRect().height > 0);
      if (!el) throw new Error('no visible .thread-content');
      el.scrollTop = el.scrollHeight - el.clientHeight;
    });
    await waitForScrollSettled(page);
    return (await edgeState(page)).gap;
  }, {
    message: 'the transcript must settle on its live edge',
    timeout: 15_000,
  }).toBeLessThanOrEqual(LIVE_EDGE_GAP_PX);
}

async function seedControlState(page: Page, threadId: string, role: string, seq: string): Promise<void> {
  await page.evaluate(({ r, tid, s }: { r: string; tid: string; s: string }) => {
    if (r === 'toggle-steps') localStorage.setItem('lucidos-steps-expanded-v2', 'false');
    else localStorage.setItem('lucidos-collapsed-exchanges', JSON.stringify([`${tid}:${s}`]));
  }, { r: role, tid: threadId, s: seq });
  await page.reload();
  await ensureOnThreadPane(page);
  await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 30_000 });
}

async function turnSeqs(page: Page): Promise<string[]> {
  return await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    const seqs = Array.from(el.querySelectorAll<HTMLElement>('.chat-exchange'))
      .map(t => t.getAttribute('data-user-seq') ?? '')
      .filter(Boolean);
    if (seqs.length === 0) throw new Error('no turn in the transcript');
    return seqs;
  });
}

async function lastTurnSeq(page: Page): Promise<string> {
  const seqs = await turnSeqs(page);
  return seqs[seqs.length - 1];
}

async function edgeState(page: Page): Promise<{ gap: number }> {
  return await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    return { gap: el.scrollHeight - el.clientHeight - el.scrollTop };
  });
}

test.describe('the step-log control holds what the reader pressed', () => {
  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  for (const { verb, gerund, to } of [
    { verb: 'show', gerund: 'showing', to: 'true' },
    { verb: 'hide', gerund: 'hiding', to: 'false' },
  ] as const) {
    test(`${gerund} the steps holds the control, not the line above it`, async ({ page }) => {
      await assertHealthy(page);
      const threadId = seedThread(`Steps control holds the press on ${verb}`);
      try {
        await openThread(page, threadId);
        const seq = await turnCarrying(page, mark(TURNS - 3, 2));
        if (verb === 'show') await pressStepsOn(page, await lastTurnSeq(page), 'false');

        await parkControlMidScreen(page, seq);
        const before = await geometry(page, seq);
        expect(
          Math.abs(before.top - before.clientHeight / 2),
          `the park left the control at ${before.top}, not halfway down`,
        ).toBeLessThanOrEqual(4);

        await pressStepsOn(page, seq, to);
        const after = await geometry(page, seq);

        expect(
          Math.abs(after.top - before.top),
          `${gerund} the steps moved the pressed control from ${before.top} to ${after.top}`,
        ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
        expect(
          Math.abs(after.scrollTop - before.scrollTop),
          'the turns above the control did not change height, so the case proves nothing',
        ).toBeGreaterThan(200);
      } finally {
        dropThread(threadId);
      }
    });
  }

  test('a thread with nothing to scroll still holds the control', async ({ page }) => {
    await assertHealthy(page);
    const threadId = seedThread('Steps control on a thread with no scroll', {
      turns: 3,
      chunks: 1,
      proseLines: 0,
      stepsPerChunk: 8,
    });
    try {
      await openThread(page, threadId);
      const seqs = await turnSeqs(page);
      expect(seqs.length, 'the short seed must render all three turns').toBe(3);

      await pressStepsOn(page, seqs[2], 'false');
      const seq = seqs[1];
      const before = await geometry(page, seq);
      expect(
        before.scrollHeight - before.clientHeight,
        `the premise is a transcript with nowhere to scroll: ${JSON.stringify(before)}`,
      ).toBeLessThanOrEqual(10);

      await pressStepsOn(page, seq, 'true');
      const after = await geometry(page, seq);

      expect(
        Math.abs(after.top - before.top),
        `showing the steps moved the pressed control from ${before.top} to ${after.top}`
        + ` (before ${JSON.stringify(before)}, after ${JSON.stringify(after)})`,
      ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
    } finally {
      dropThread(threadId);
    }
  });

  for (const c of [
    { role: 'toggle-steps', name: 'the step log', off: 'false', on: 'true' },
    { role: 'toggle-collapsed', name: 'the fold', off: 'true', on: 'false' },
  ] as const) {
    test(`${c.name} holds its control for a reader on the end of the thread`, async ({ page }) => {
      await assertHealthy(page);
      const threadId = seedThread(`Live edge holds ${c.role}`, {
        stepsPerChunk: STEPS_PER_RUN,
        chunks: 2,
        proseLines: 1,
      });
      try {
        await openThread(page, threadId, { disarm: false, renderAll: false });
        const seq = await lastTurnSeq(page);
        await seedControlState(page, threadId, c.role, seq);
        await expect(controlOn(page, seq, c.role)).toHaveAttribute('aria-pressed', c.off);
        await parkAtLiveEdge(page);

        const edgeBefore = await edgeState(page);
        expect(edgeBefore.gap, 'the reader must start on the end of the thread').toBeLessThanOrEqual(LIVE_EDGE_GAP_PX);
        const before = await geometry(page, seq, c.role);
        expect(
          before.top,
          `the control is off screen at ${before.top}, so no finger could reach it`,
        ).toBeGreaterThan(0);
        expect(before.top).toBeLessThan(before.clientHeight);

        await pressControlOn(page, seq, c.role, c.on);
        const after = await geometry(page, seq, c.role);

        expect(
          Math.abs(after.top - before.top),
          `the press moved its own control from ${before.top} to ${after.top}`
          + ` (before ${JSON.stringify(before)}, after ${JSON.stringify(after)})`,
        ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
        expect(
          after.scrollHeight - before.scrollHeight,
          'the press grew the transcript by nothing, so the case proves nothing',
        ).toBeGreaterThan(after.clientHeight);
      } finally {
        dropThread(threadId);
      }
    });
  }

  test('the step log holds its control when it was pressed a moment ago', async ({ page }) => {
    await assertHealthy(page);
    const threadId = seedThread('Live edge clamp round trip', {
      stepsPerChunk: 4,
      chunks: 1,
      proseLines: 0,
      turns: 14,
    });
    try {
      await openThread(page, threadId, { renderAll: false });
      const seq = await lastTurnSeq(page);
      await expect(controlOn(page, seq)).toHaveAttribute('aria-pressed', 'true');
      await parkAtLiveEdge(page);
      expect(
        (await edgeState(page)).gap,
        'the reader must start on the end of the thread',
      ).toBeLessThanOrEqual(LIVE_EDGE_GAP_PX);

      const beforeHide = await geometry(page, seq);
      expect(
        beforeHide.top,
        `the control is off screen at ${beforeHide.top}, so no finger could reach it`,
      ).toBeGreaterThan(0);
      expect(beforeHide.top).toBeLessThan(beforeHide.clientHeight);

      await pressStepsOn(page, seq, 'false');
      const clamped = await geometry(page, seq);
      expect(
        clamped.top - beforeHide.top,
        `the hide did not clamp: the control sat at ${beforeHide.top} and now sits at ${clamped.top}`,
      ).toBeGreaterThan(DRIFT_TOLERANCE_PX);

      await pressStepsOn(page, seq, 'true');
      const after = await geometry(page, seq);

      expect(
        Math.abs(after.top - clamped.top),
        `the press moved its own control from ${clamped.top} to ${after.top}`
        + ` (before ${JSON.stringify(clamped)}, after ${JSON.stringify(after)})`,
      ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
    } finally {
      dropThread(threadId);
    }
  });
});
