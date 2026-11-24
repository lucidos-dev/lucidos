import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy, disarmFollowSeed } from './helpers';
import { psql } from './db-helpers';

test.use({ reducedMotion: 'no-preference' });


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
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', '${title}', 'claude_code', now(), ${steps.length}, false, true, 'idle', 'active', 'active', true, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return threadId;
}

async function rowAtTop(page: Page): Promise<{ label: string; relTop: number } | null> {
  return page.locator('.thread-content').first().evaluate((el) => {
    const top = el.getBoundingClientRect().top - new DOMMatrix(getComputedStyle(el).transform).m42;
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

async function rowOffset(page: Page, label: string): Promise<number | null> {
  return page.locator('.thread-content').first().evaluate(rowOffsetIn, label);
}

function rowOffsetIn(el: HTMLElement, label: string): number | null {
  const top = el.getBoundingClientRect().top - new DOMMatrix(getComputedStyle(el).transform).m42;
  for (const row of Array.from(el.querySelectorAll<HTMLElement>('[data-row-event]'))) {
    const text = (row.textContent ?? '').replace(/\s+/g, ' ').trim();
    if (!text.startsWith(label) || /\d/.test(text.charAt(label.length))) continue;
    const rect = row.getBoundingClientRect();
    if (rect.height > 0) return Math.round(rect.top - top);
  }
  return null;
}

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

    for (let i = 0; i < 80; i++) {
      const at = await rowAtTop(page);
      if (at && /Run t3s([0-9]|[1-5][0-9])\b/.test(at.label)) break;
      await scrollUp(page, 400);
    }
    await page.waitForTimeout(600);
    const parked = await rowAtTop(page);
    expect(parked?.label, 'parked deep inside the big turn').toMatch(/Run t3s\d+/);

    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadB);
    await expect(transcript.getByText('Run t0s0').first()).toBeVisible();
    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadA);
    const near = async () => Math.abs((await rowOffset(page, parked!.label) ?? Infinity) - parked!.relTop) <= 2;
    await expect.poll(near, { timeout: 10_000 }).toBe(true);
    await page.waitForTimeout(1500);
    expect(await near()).toBe(true);

    await page.addInitScript(([fn, label]) => {
      const rowOffsetIn = new Function(`return ${fn}`)() as (el: HTMLElement, label: string) => number | null;
      const frames: Array<number | null> = [];
      (window as unknown as { __rowFrames: typeof frames }).__rowFrames = frames;
      const sample = () => {
        const el = document.querySelector<HTMLElement>('.thread-content');
        if (el && el.getBoundingClientRect().height > 0) frames.push(rowOffsetIn(el, label));
        requestAnimationFrame(sample);
      };
      requestAnimationFrame(sample);
    }, [rowOffsetIn.toString(), parked!.label] as const);
    await page.reload();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();
    await expect.poll(near, { timeout: 15_000 }).toBe(true);
    await page.waitForTimeout(1500);
    const frames = await page.evaluate(() => (window as unknown as { __rowFrames: Array<number | null> }).__rowFrames);
    const isParked = (f: number | null) => f !== null && Math.abs(f - parked!.relTop) <= 2;
    const landed = frames.findIndex(isParked);
    expect(landed, 'a frame shows the parked row where it was left').toBeGreaterThanOrEqual(0);
    expect(frames.slice(landed).filter((f) => !isParked(f)), 'once the parked row is on its line it stays there').toEqual([]);
    const stillScrollable = await transcript.evaluate((el) => el.scrollTop < el.scrollHeight - el.clientHeight - 1);
    expect(stillScrollable, 'never parked at the bottom').toBe(true);
  });

  test('growth above a restored row leaves the reader on it', async ({ page }) => {
    test.setTimeout(120_000);
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

    await transcript.evaluate((el) => {
      el.dispatchEvent(new WheelEvent('wheel', { deltaY: 1500, bubbles: true }));
      el.scrollTop += 1500;
    });
    await page.waitForTimeout(600);
    const parked = await rowAtTop(page);
    expect(parked, 'a row rests on the top line').not.toBeNull();

    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadB);
    await expect(transcript.locator('.chat-exchange')).toHaveCount(3);
    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadA);
    await expect.poll(async () => (await rowAtTop(page))?.label, { timeout: 10_000 }).toBe(parked!.label);

    await transcript.evaluate((el) => {
      el.querySelector<HTMLElement>('.chat-exchange')!.style.paddingTop = '900px';
    });
    await page.waitForTimeout(300);
    const after = await rowAtTop(page);
    expect(after?.label, 'the same row stays on the top line').toBe(parked!.label);
    expect(Math.abs(after!.relTop - parked!.relTop)).toBeLessThanOrEqual(2);
  });

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
    await transcript.evaluate((el) => {
      const row = Array.from(el.querySelectorAll<HTMLElement>('[data-row-event]'))
        .find((r) => (r.textContent ?? '').includes('Run t0s5'));
      const by = Math.round(row!.getBoundingClientRect().top - el.getBoundingClientRect().top) + 4;
      el.dispatchEvent(new WheelEvent('wheel', { deltaY: by, bubbles: true }));
      el.scrollTop += by;
    });
    await page.waitForTimeout(600);
    const parked = await rowAtTop(page);
    expect(parked?.label, 'parked inside the first turn').toMatch(/Run t0s\d+/);

    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadB);
    await expect(transcript.locator('.chat-exchange')).toHaveCount(3);

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
