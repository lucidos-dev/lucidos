import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, disarmFollowSeed, hideDashboardRail } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';


const MAX_DRIFT_PX = 0.05;

const TURNS = 12;

function q(o: unknown): string {
  return JSON.stringify(o).replace(/'/g, "''");
}

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
    await new Promise(r => setTimeout(r, 600));
    if (!btn.isConnected) return null;
    return {
      drift: btn.getBoundingClientRect().top - before,
      heightChange: tc.scrollHeight - heightBefore,
    };
  });
}

async function clearToasts(page: Page): Promise<void> {
  await page.evaluate(() => {
    document.querySelectorAll<HTMLButtonElement>('.toast .toast-close').forEach(b => b.click());
  });
  await page.locator('.toast-container .toast').first().waitFor({ state: 'detached', timeout: 5_000 }).catch(() => {
  });
}

async function park(page: Page, frac: number): Promise<void> {
  await page.locator('.thread-content.visible:visible').first().evaluate((el, f) => {
    el.scrollTop = Math.round((el.scrollHeight - el.clientHeight) * (f as number));
  }, frac);
  await page.waitForTimeout(300);
}

test.describe('A turn control holds itself still', () => {
  test("the full-response toggle leaves the control where it was, at a fractional root font size", async ({ page }) => {
    await assertHealthy(page);
    const threadId = seedThread('Turn control anchor');
    try {
      await disarmFollowSeed(page);
      await hideDashboardRail(page);
      await navigateToApp(page);
      await openThreadDrawer(page);
      await clearToasts(page);
      await page.locator('.thread-row:has-text("Turn control anchor")').first().click();
      await ensureOnThreadPane(page);

      const tc = page.locator('.thread-content.visible:visible').first();
      await expect
        .poll(() => tc.evaluate(el => el.querySelectorAll('.chat-exchange').length), { timeout: 25_000 })
        .toBe(TURNS);
      await expect
        .poll(() => tc.evaluate(el => el.scrollHeight - el.clientHeight), { timeout: 10_000 })
        .toBeGreaterThan(500);

      let measured = 0;
      for (const scale of ['105%', '112.5%', '137.5%']) {
        await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', s), scale);
        await page.waitForTimeout(500);
        const rootPx = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize));

        for (let press = 1; press <= 4; press++) {
          await park(page, 0.3 + (press % 3) * 0.15);
          const m = await pressAndMeasure(page);
          if (!m) continue;
          measured++;
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
