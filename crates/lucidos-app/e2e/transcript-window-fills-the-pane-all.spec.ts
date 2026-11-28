import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy } from './helpers';
import { psql } from './db-helpers';


const TOOL_CALLS = 120;

type RowWriter = (type: string, payload: string) => string;

function callPair(row: RowWriter, messageId: string, useId: string, n: number | string): string[] {
  return [
    row('CodingAgentToolCalled',
      `{"name":"Bash","args":{"command":"echo ${n}"},"description":"Run echo ${n}",` +
      `"channel":"claude_code","tool_use_id":"${useId}","coding_agent":"claude-code",` +
      `"request_event_id":"${messageId}"}`),
    row('CodingAgentToolResult',
      `{"name":"","result":"${n} done","channel":"claude_code","tool_use_id":"${useId}",` +
      `"coding_agent":"claude-code","request_event_id":"${messageId}"}`),
  ];
}

function seedBoundaryBehindHugeTurn(): string {
  const threadId = randomUUID();
  const messageId = randomUUID();
  const now = new Date().toISOString();

  const row: RowWriter = (type, payload) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;

  const steps: string[] = [];
  for (let i = 0; i < TOOL_CALLS; i++) steps.push(...callPair(row, messageId, `e2e-tool-${i}`, i));

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E windowed transcript', 'claude_code', '${now}', 1, false, true, 'idle', 'archived', 'active', true, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + [
      `('${messageId}', 'MessageReceived', '{"text":"do the work","mode":"human","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      ...steps,
      row('ResponseGenerated', `{"text":"Done.","images":[],"request_event_id":"${messageId}"}`),
      row('ChangeApplied', `{"change_id":"${randomUUID()}","commits":["fix: e2e"],"client_update":false}`),
    ].join(',\n'),
  ].join(';\n'));

  return threadId;
}

const PROMPT_TEXT = 'do the work';

const TOOL_CALLS_PAST_A_PAGE = 260;

function seedTurnLongerThanAPage(): string {
  const threadId = randomUUID();
  const messageId = randomUUID();
  const now = new Date().toISOString();

  const row: RowWriter = (type, payload) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;

  const steps: string[] = [];
  for (let i = 0; i < TOOL_CALLS_PAST_A_PAGE; i++) {
    steps.push(...callPair(row, messageId, `e2e-paged-tool-${i}`, i));
  }

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E paged transcript', 'claude_code', '${now}', 1, false, true, 'idle', 'archived', 'active', true, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + [
      `('${messageId}', 'MessageReceived', '{"text":"${PROMPT_TEXT}","mode":"human","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      ...steps,
      row('ResponseGenerated', `{"text":"Done.","images":[],"request_event_id":"${messageId}"}`),
    ].join(',\n'),
  ].join(';\n'));

  return threadId;
}

const WALK_TURNS = 12;
const WALK_CALLS_PER_TURN = 100;

function seedThreadSeveralPagesLong(): string {
  const threadId = randomUUID();
  const base = Date.now();
  let n = 0;
  const at = () => new Date(base + n++ * 1000).toISOString();
  const rows: string[] = [];
  const row: RowWriter = (type, payload) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;

  for (let t = 0; t < WALK_TURNS; t++) {
    const messageId = randomUUID();
    const text = t === 0 ? FIRST_TURN_TEXT : `turn ${t}`;
    rows.push(`('${messageId}', 'MessageReceived', '{"text":"${text}","mode":"human","channel":"claude_code"}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`);
    for (let i = 0; i < WALK_CALLS_PER_TURN; i++) {
      rows.push(...callPair(row, messageId, `walk-${t}-${i}`, `${t}.${i}`));
    }
    rows.push(row('ResponseGenerated', `{"text":"Done ${t}.","images":[],"request_event_id":"${messageId}"}`));
  }

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E multi-page transcript', 'claude_code', '${new Date(base).toISOString()}', 1, false, true, 'idle', 'archived', 'active', true, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return threadId;
}

const FIRST_TURN_TEXT = 'the very first thing asked';

const REPORTED_BIG_TURN_CALLS = 184;
const REPORTED_LAST_TURN_CALLS = 24;

const LAST_TURN_TEXT = 'and then this one';

function seedFragmentUnderOneShortTurn(): string {
  const threadId = randomUUID();
  const base = Date.now();
  let n = 0;
  const at = () => new Date(base + n++ * 1000).toISOString();
  const rows: string[] = [];
  const row: RowWriter = (type, payload) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;

  const turn = (text: string, calls: number, tag: string, done: boolean) => {
    const messageId = randomUUID();
    rows.push(`('${messageId}', 'MessageReceived', '{"text":"${text}","mode":"human","channel":"claude_code"}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`);
    for (let i = 0; i < calls; i++) rows.push(...callPair(row, messageId, `${tag}-${i}`, i));
    if (done) rows.push(row('ResponseGenerated', `{"text":"Done.","images":[],"request_event_id":"${messageId}"}`));
  };

  turn(FIRST_TURN_TEXT, REPORTED_BIG_TURN_CALLS, 'big', false);
  turn(LAST_TURN_TEXT, REPORTED_LAST_TURN_CALLS, 'last', true);

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E fragment under one short turn', 'claude_code', '${new Date(base).toISOString()}', 2, false, true, 'idle', 'archived', 'active', true, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return threadId;
}

const SILENT_TURN_TEXT = 'the silent turn began here';

const PROSE_CHUNKS = 30;
const CALLS_PER_CHUNK = 2;

const SILENT_CALLS = 700;

const PROSE_MARK = 'Prose chunk';

function seedSilentTurn(): string {
  const threadId = randomUUID();
  const messageId = randomUUID();
  const base = Date.now();
  let n = 0;
  const at = () => new Date(base + n++ * 1000).toISOString();
  const rows: string[] = [];
  const row: RowWriter = (type, payload) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;
  const text = (md: string) => row('CodingAgentTextStreamed',
    `{"text":"${md}","channel":"claude_code","coding_agent":"claude-code","request_event_id":"${messageId}"}`);
  let call = 0;
  const silentCall = () => {
    rows.push(text('\\n\\n'), ...callPair(row, messageId, `silent-${call}`, call));
    call++;
  };

  rows.push(`('${messageId}', 'MessageReceived', '{"text":"${SILENT_TURN_TEXT}","mode":"human","channel":"claude_code"}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`);
  for (let c = 0; c < PROSE_CHUNKS; c++) {
    rows.push(text(`\\n\\n${PROSE_MARK} ${c}. The agent explains what it will change next, ` +
      'over enough words to wrap onto a second line on a phone.'));
    for (let i = 0; i < CALLS_PER_CHUNK; i++) silentCall();
  }
  for (let i = 0; i < SILENT_CALLS; i++) silentCall();

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E silent turn', 'claude_code', '${new Date(base).toISOString()}', 1, false, true, 'idle', 'archived', 'active', true, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return threadId;
}

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  await navigateToApp(page);
}

async function stepUp(page: Page, jump: number, wheelless: boolean): Promise<void> {
  if (!wheelless) {
    await page.mouse.wheel(0, -jump);
    return;
  }
  await page.locator('.thread-content').first().evaluate((el, d) => {
    el.dispatchEvent(new WheelEvent('wheel', { deltaY: -d, bubbles: true }));
    el.scrollTop = Math.max(0, el.scrollTop - d);
  }, jump);
}

async function walkUpUntilHeld(page: Page, text: string, wheelless: boolean): Promise<boolean> {
  const transcript = page.locator('.thread-content').first();
  const box = (await transcript.boundingBox())!;
  const jump = Math.round(box.height * 0.8);
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  for (let step = 0; step < 200; step++) {
    await stepUp(page, jump, wheelless);
    await page.waitForTimeout(60);
    if (await transcript.evaluate((el, t) => (el.textContent ?? '').includes(t), text)) return true;
  }
  return false;
}

const DRIFT_TOLERANCE_PX = 24;

const DRIFT_MAX_STEPS = 200;
const DRIFT_SETTLE_MS = 400;

async function readUpAndMeasureDrift(
  page: Page, text: string,
): Promise<{ reached: boolean; jumps: string[] }> {
  const transcript = page.locator('.thread-content').first();
  const jumps: string[] = [];
  for (let step = 0; step < DRIFT_MAX_STEPS; step++) {
    const taken = await transcript.evaluate((el) => {
      const pinned = (n: HTMLElement) => {
        for (let a: HTMLElement | null = n; a && a !== el; a = a.parentElement) {
          if (/sticky|fixed/.test(getComputedStyle(a).position)) return true;
        }
        return false;
      };
      const leaves = [...el.querySelectorAll<HTMLElement>('.chat-exchange *')].filter(
        (n) => n.childElementCount === 0 && (n.textContent ?? '').trim().length > 2 && !pinned(n),
      );
      const count = new Map<string, number>();
      for (const n of leaves) {
        const t = n.textContent!.trim();
        count.set(t, (count.get(t) ?? 0) + 1);
      }
      const box = el.getBoundingClientRect();
      const mid = box.top + box.height / 2;
      let probe: HTMLElement | null = null;
      for (const n of leaves) {
        if (count.get(n.textContent!.trim()) !== 1) continue;
        const r = n.getBoundingClientRect();
        if (r.height === 0) continue;
        if (!probe || Math.abs(r.top - mid) < Math.abs(probe.getBoundingClientRect().top - mid)) probe = n;
      }
      const before = el.scrollTop;
      const top0 = probe?.getBoundingClientRect().top ?? 0;
      const d = Math.round(el.clientHeight * 0.3);
      el.dispatchEvent(new WheelEvent('wheel', { deltaY: -d, bubbles: true }));
      el.scrollTop = Math.max(0, el.scrollTop - d);
      const moved = before - el.scrollTop;
      return { text: probe?.textContent?.trim() ?? null, expected: top0 + moved, before };
    });
    await page.waitForTimeout(DRIFT_SETTLE_MS);
    const after = await transcript.evaluate((el, probeText) => {
      const matches = probeText
        ? [...el.querySelectorAll<HTMLElement>('.chat-exchange *')].filter(
          (n) => n.childElementCount === 0 && n.textContent?.trim() === probeText)
        : [];
      const top = matches.length === 1 ? matches[0].getBoundingClientRect().top : null;
      return { top, scrollTop: el.scrollTop, scrollHeight: el.scrollHeight };
    }, taken.text);
    const where = `(scrollTop ${taken.before} -> ${after.scrollTop}, scrollHeight ${after.scrollHeight})`;
    if (taken.text && after.top === null) {
      jumps.push(`step ${step}: "${taken.text}" left the transcript ${where}`);
    } else if (taken.text && after.top !== null && Math.abs(after.top - taken.expected) > DRIFT_TOLERANCE_PX) {
      jumps.push(`step ${step}: "${taken.text}" off by ${Math.round(after.top - taken.expected)}px ${where}`);
    }
    if (await transcript.evaluate((el, t) => el.scrollTop <= 1 && (el.textContent ?? '').includes(t), text)) {
      return { reached: true, jumps };
    }
  }
  return { reached: false, jumps };
}

test.describe('Windowed transcript', () => {
  const seededThreads: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    seededThreads.length = 0;
  });

  test.afterEach(() => {
    if (seededThreads.length === 0) return;
    const ids = seededThreads.map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  test('fills the pane when the seeded slice cannot', async ({ page }) => {
    const threadId = seedBoundaryBehindHugeTurn();
    seededThreads.push(threadId);
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    await expect.poll(
      () => transcript.evaluate(el => el.scrollHeight - el.clientHeight),
      { message: 'the transcript must be scrollable while turns sit above the window' },
    ).toBeGreaterThan(10);
    await expect(transcript.locator('.chat-exchange')).toHaveCount(2);
  });

  test('opens on a page that carries no turn boundary', async ({ page }) => {
    const threadId = seedTurnLongerThanAPage();
    seededThreads.push(threadId);
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    await expect(page.locator('.thread-empty-state')).toHaveCount(0);
    await expect.poll(
      () => transcript.evaluate(el => el.scrollHeight - el.clientHeight),
      { message: 'a page with no boundary must still fill the pane' },
    ).toBeGreaterThan(10);

    const chevron = page.locator('.scroll-to-top.visible').first();
    await expect(chevron).toBeVisible();
    await chevron.click();

    const prompt = transcript.getByText(PROMPT_TEXT);
    await expect(prompt).toBeVisible({ timeout: 30_000 });
    await expect.poll(
      () => transcript.evaluate(el => el.scrollTop),
      { message: 'the chevron must land the reader at the top', timeout: 30_000 },
    ).toBeLessThan(200);
    const box = await prompt.boundingBox();
    const pane = await transcript.boundingBox();
    expect(box, 'the first message must be on screen').not.toBeNull();
    expect(box!.y).toBeLessThan(pane!.y + pane!.height);
  });

  test('walks to the top of a thread several pages long', async ({ page, browserName, isMobile }) => {
    test.setTimeout(180_000);
    const threadId = seedThreadSeveralPagesLong();
    seededThreads.push(threadId);
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible({ timeout: 60_000 });

    const wheelless = browserName === 'webkit' && isMobile;
    const reached = await walkUpUntilHeld(page, FIRST_TURN_TEXT, wheelless);
    expect(reached, 'walking up must reach the thread\'s first message').toBe(true);
  });

  test('reading up a paged thread never moves the reader', async ({ page }) => {
    test.setTimeout(180_000);
    const threadId = seedThreadSeveralPagesLong();
    seededThreads.push(threadId);
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible({ timeout: 60_000 });
    const { reached, jumps } = await readUpAndMeasureDrift(page, FIRST_TURN_TEXT);
    expect(jumps, 'no grow or page landing may move the reader off their content').toEqual([]);
    expect(reached, 'reading up must reach the thread\'s first message').toBe(true);
  });

  test('reaches what is above a page that folds to one short turn', async ({ page, browserName, isMobile }) => {
    test.setTimeout(180_000);
    const threadId = seedFragmentUnderOneShortTurn();
    seededThreads.push(threadId);
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.getByText(LAST_TURN_TEXT).first()).toBeVisible({ timeout: 60_000 });

    await expect.poll(
      () => transcript.evaluate(el => el.scrollHeight - el.clientHeight),
      { message: 'the reader must have something to scroll' },
    ).toBeGreaterThan(10);

    const wheelless = browserName === 'webkit' && isMobile;
    const reached = await walkUpUntilHeld(page, FIRST_TURN_TEXT, wheelless);
    expect(reached, 'the wheel alone must reach the thread\'s first message').toBe(true);
  });

  test('scrolls a silent turn with steps hidden', async ({ page, browserName, isMobile }) => {
    test.setTimeout(180_000);
    const threadId = seedSilentTurn();
    seededThreads.push(threadId);
    await page.addInitScript(() => localStorage.setItem('lucidos-steps-expanded-v2', 'false'));
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible({ timeout: 60_000 });

    await expect.poll(
      () => transcript.evaluate(el => el.scrollHeight - el.clientHeight),
      { message: 'a silent turn must still leave the reader something to scroll', timeout: 60_000 },
    ).toBeGreaterThan(10);
    await expect(transcript.getByText(PROSE_MARK).first()).toBeVisible();

    const wheelless = browserName === 'webkit' && isMobile;
    const reached = await walkUpUntilHeld(page, SILENT_TURN_TEXT, wheelless);
    expect(reached, 'the wheel alone must reach the turn\'s opening message').toBe(true);
  });
});
