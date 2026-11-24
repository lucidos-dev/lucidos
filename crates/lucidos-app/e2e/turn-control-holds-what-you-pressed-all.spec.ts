import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, enableMobileDynamicBars, disarmFollowSeed, disableMobileDynamicBars, ensureOnThreadPane, navigateToApp, renderWholeTranscript, waitForScrollSettled } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';


const TURNS = 6;
const CHUNKS = 10;
const STEPS_PER_CHUNK = 4;
const PARK_FRACTIONS = [0.05, 0.35, 0.65, 0.95];
const TOLERANCE_PX = 4;
const PARK_TOLERANCE_PX = 8;

function q(o: unknown): string {
  return JSON.stringify(o).replace(/'/g, "''");
}

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
  await disarmFollowSeed(page);
  await enableMobileDynamicBars(page);
  await navigateToApp(page);
  await ensureOnThreadPane(page);
  await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 30_000 });
  await expect(page.locator('[data-role="inline-step"]:visible').first()).toBeVisible({ timeout: 30_000 });
  await renderWholeTranscript(page);
}

async function turnSeqs(page: Page): Promise<string[]> {
  return await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    return Array.from(el.querySelectorAll<HTMLElement>('.chat-exchange'))
      .map(t => t.getAttribute('data-user-seq') ?? '')
      .filter(Boolean);
  });
}

function control(page: Page, seq: string, role: string) {
  return page.locator(`.chat-exchange[data-user-seq="${seq}"] [data-role="${role}"]`).first();
}

interface Geometry {
  top: number;
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  inset: number;
}

async function geometry(page: Page, seq: string, role: string): Promise<Geometry> {
  return await page.evaluate(({ s, r }: { s: string; r: string }) => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0)!;
    const btn = el.querySelector<HTMLElement>(`.chat-exchange[data-user-seq="${s}"] [data-role="${r}"]`);
    if (!btn) throw new Error(`no ${r} control on turn ${s}`);
    const containerTop = el.getBoundingClientRect().top;
    let edge = containerTop;
    for (const child of Array.from(el.children) as HTMLElement[]) {
      if (!child.matches('[data-scroller-pinned]')) continue;
      const r = child.getBoundingClientRect();
      if (r.height > 0 && r.bottom > edge) edge = r.bottom;
    }
    return {
      top: btn.getBoundingClientRect().top - containerTop,
      scrollTop: el.scrollTop,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
      inset: edge - containerTop,
    };
  }, { s: seq, r: role });
}

async function controlTop(page: Page, seq: string, role: string): Promise<number> {
  return (await geometry(page, seq, role)).top;
}

function correctionWasReachable(after: Geometry, drift: number): boolean {
  const max = Math.max(0, after.scrollHeight - after.clientHeight);
  const target = after.scrollTop + drift;
  return target >= -1 && target <= max + 1;
}

async function parkControlAt(page: Page, seq: string, role: string, fraction: number): Promise<number> {
  const want = await page.evaluate((f: number) =>
    Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0)!.clientHeight * f, fraction);
  for (let round = 0; round < 4; round++) {
    const at = await controlTop(page, seq, role);
    if (Math.abs(at - want) <= 1) break;
    await page.evaluate((d: number) => {
      const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
        .find(c => c.getBoundingClientRect().height > 0)!;
      el.scrollTop += d;
    }, at - want);
    await waitForScrollSettled(page);
  }
  return want;
}

async function press(page: Page, seq: string, role: string, expected: 'true' | 'false'): Promise<void> {
  const btn = control(page, seq, role);
  await btn.dispatchEvent('click');
  await expect(btn).toHaveAttribute('aria-pressed', expected);
  await waitForScrollSettled(page);
}

async function ensureState(page: Page, seq: string, role: string, state: 'true' | 'false'): Promise<void> {
  const btn = control(page, seq, role);
  if (await btn.getAttribute('aria-pressed') !== state) await press(page, seq, role, state);
  await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0)!;
    el.scrollTop = 0;
  });
  await waitForScrollSettled(page);
}

const CONTROLS = [
  { role: 'toggle-steps', name: 'the steps', parks: PARK_FRACTIONS },
  { role: 'toggle-details', name: 'the full response', parks: PARK_FRACTIONS },
  { role: 'toggle-collapsed', name: 'the fold', parks: PARK_FRACTIONS },
] as const;

test.describe('a turn control holds the element the reader clicked', () => {
  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  for (const c of CONTROLS) {
    for (const from of ['true', 'false'] as const) {
      const to = from === 'true' ? 'false' : 'true';
      test(`turning ${c.name} ${from === 'true' ? 'off' : 'on'}, swept up the screen`, async ({ page }) => {
        await assertHealthy(page);
        const threadId = seedThread(`Sweep ${c.role} ${from}`);
        const failures: string[] = [];
        let held = 0;
        let clamped = 0;
        try {
          await openThread(page, threadId);
          const seqs = await turnSeqs(page);
          expect(seqs.length, 'the seeded thread must render every turn').toBe(TURNS);

          for (const seq of seqs) {
            for (const fraction of c.parks) {
              await ensureState(page, seq, c.role, from);
              const want = await parkControlAt(page, seq, c.role, fraction);
              const before = await geometry(page, seq, c.role);
              if (Math.abs(before.top - want) > PARK_TOLERANCE_PX) continue;

              await press(page, seq, c.role, to);
              const after = await geometry(page, seq, c.role);
              const drift = after.top - before.top;
              if (Math.abs(drift) > TOLERANCE_PX && !correctionWasReachable(after, drift)) {
                clamped++;
                continue;
              }
              if (before.top >= before.inset && after.top < after.inset) {
                failures.push(
                  `turn ${seq} at ${Math.round(fraction * 100)}%: the chrome buried the`
                  + ` control, its readable edge going ${before.inset} to ${after.inset}`
                  + ` with the control at ${after.top.toFixed(1)}`
                  + ` (drift ${drift.toFixed(1)}, before ${JSON.stringify(before)},`
                  + ` after ${JSON.stringify(after)})`,
                );
                continue;
              }
              if (Math.abs(drift) <= TOLERANCE_PX) { held++; continue; }
              failures.push(
                `turn ${seq} (${seqs.indexOf(seq) + 1} of ${seqs.length})`
                + ` at ${Math.round(fraction * 100)}%: the control moved ${drift.toFixed(1)}px,`
                + ` from ${before.top.toFixed(1)} to ${after.top.toFixed(1)}`
                + ` (before ${JSON.stringify(before)}, after ${JSON.stringify(after)})`,
              );
            }
          }
          expect(held + failures.length, 'no press was reachable, so the sweep asserted nothing')
            .toBeGreaterThan(TURNS);
          expect(
            failures.join('\n'),
            `${failures.length} of ${held + failures.length} reachable presses moved the control`
            + ` (${clamped} more were clamped)`,
          ).toBe('');
        } finally {
          dropThread(threadId);
        }
      });
    }
  }
});
