import { test, expect, Page } from './fixtures';
import { navigateToApp, assertHealthy, disarmFollowSeed } from './helpers';
import { psql, seedStepHeavyThread as seedStepHeavyThreadRows } from './db-helpers';

test.use({ reducedMotion: 'no-preference' });


const TURNS = 24;
const STEPS_PER_TURN = 8;

const EVENTS_PER_TURN = 2 + STEPS_PER_TURN * 2;

const PAGE_SIZE = 400;

const MAX_READING_CHASES = 2;
const CHASE_PAGE_SIZE = 2000;

const TURNS_BEHIND_THE_PAGE =
  Math.ceil(Math.max(0, TURNS * EVENTS_PER_TURN - PAGE_SIZE) / EVENTS_PER_TURN);

const PARK_ON_TURN = 4;

const CHASE_WINDOW_MS = 8_000;

function seedStepHeavyThread(turns = TURNS): { threadId: string; messageIds: string[] } {
  return seedStepHeavyThreadRows({ turns, stepsPerTurn: STEPS_PER_TURN, title: 'E2E reopen on the same turn' });
}

function scrollKey(threadId: string): string {
  return `lucidos-scroll-thread-${threadId}`;
}

async function openThread(page: Page, threadId: string, reading?: string): Promise<void> {
  await page.addInitScript(([tid, key, rec]: [string, string, string | null]) => {
    localStorage.setItem('lucidos-focused-thread', tid);
    if (rec !== null) localStorage.setItem(key, rec);
  }, [threadId, scrollKey(threadId), reading ?? null] as [string, string, string | null]);
  await disarmFollowSeed(page);
  await navigateToApp(page);
}

async function restingOn(page: Page): Promise<{ id: string | null; relTop: number }> {
  return page.locator('.thread-content').first().evaluate(async (el) => {
    for (let frame = 0; frame < 60 && el.style.transform.includes('translateZ'); frame++) {
      await new Promise(requestAnimationFrame);
    }
    const top = el.getBoundingClientRect().top;
    const turns = Array.from(el.querySelectorAll<HTMLElement>('.chat-exchange'));
    let earliest: { id: string | null; relTop: number } | null = null;
    for (let i = turns.length - 1; i >= 0; i--) {
      const rect = turns[i].getBoundingClientRect();
      if (rect.height <= 0) continue;
      const relTop = Math.round(rect.top - top) + 0;
      const id = turns[i].getAttribute('data-event-id');
      if (relTop <= 0) return { id, relTop };
      earliest = { id, relTop };
    }
    return earliest ?? { id: null, relTop: 0 };
  });
}

async function nudge(page: Page, by: number): Promise<void> {
  await page.locator('.thread-content').first().evaluate((el, delta) => {
    el.dispatchEvent(new WheelEvent('wheel', { deltaY: delta, bubbles: true }));
    el.scrollTop = Math.max(0, el.scrollTop + delta);
  }, by);
  await page.waitForTimeout(150);
}

const PARK_ABOVE_THE_LINE_PX = 8;

async function parkOn(page: Page, eventId: string): Promise<void> {
  await page.locator('.thread-content').first().evaluate((el, [id, above]: [string, number]) => {
    const turn = el.querySelector<HTMLElement>(`[data-event-id="${id}"]`);
    if (!turn) throw new Error(`the turn to park on is not rendered: ${id}`);
    el.scrollTop += turn.getBoundingClientRect().top - el.getBoundingClientRect().top + above;
  }, [eventId, PARK_ABOVE_THE_LINE_PX] as [string, number]);
  await page.waitForTimeout(150);
}

async function restingTurn(page: Page, messageIds: string[]): Promise<number> {
  const at = await restingOn(page);
  return messageIds.indexOf(at.id ?? '');
}

function isOlderHistoryRead(url: string, threadId: string): boolean {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  if (!parsed.pathname.endsWith(`/threads/${threadId}/events`)) return false;
  if (parsed.searchParams.has('before_created')) return true;
  return !parsed.searchParams.has('limit') && !parsed.searchParams.has('after');
}

test.describe('Where a thread reopens', () => {
  const seededThreads: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    seededThreads.length = 0;
    expect(TURNS * EVENTS_PER_TURN, 'the seed must be longer than one page')
      .toBeGreaterThan(PAGE_SIZE);
    expect(PARK_ON_TURN, 'the parked turn must sit inside the newest page')
      .toBeGreaterThanOrEqual(TURNS_BEHIND_THE_PAGE);
  });

  test.afterEach(() => {
    if (seededThreads.length === 0) return;
    const ids = seededThreads.map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  test('a reload lands on the same turn, not the top of a re-seeded window', async ({ page }) => {
    test.setTimeout(180_000);
    const { threadId, messageIds } = seedStepHeavyThread();
    seededThreads.push(threadId);

    await openThread(page, threadId);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    await expect(transcript.locator(`[data-event-id="${messageIds[TURNS - 1]}"]`)).toHaveCount(1);
    await expect(transcript.locator(`[data-event-id="${messageIds[PARK_ON_TURN]}"]`)).toHaveCount(0);

    await nudge(page, 100_000);

    await expect.poll(
      async () => {
        await nudge(page, -2400);
        return transcript.locator(`[data-event-id="${messageIds[0]}"]`).count();
      },
      { message: 'scrolling up must reach the first turn', timeout: 90_000 },
    ).toBe(1);

    await expect.poll(
      async () => {
        await parkOn(page, messageIds[PARK_ON_TURN]);
        return restingTurn(page, messageIds);
      },
      { message: 'the reader must come to rest on the turn they are parked on', timeout: 30_000 },
    ).toBe(PARK_ON_TURN);

    await page.waitForTimeout(600);
    const parked = await restingOn(page);
    expect(messageIds.indexOf(parked.id ?? ''), 'the park must hold while the save settles')
      .toBe(PARK_ON_TURN);
    const recorded = await page.evaluate(
      (key) => localStorage.getItem(key), scrollKey(threadId));
    expect(recorded).toMatch(
      new RegExp(`^line:-?\\d+:${parked.relTop}:${parked.id}:`));

    let olderReads = 0;
    page.on('request', req => {
      if (isOlderHistoryRead(req.url(), threadId)) olderReads += 1;
    });
    await page.reload();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    await expect.poll(
      () => restingTurn(page, messageIds),
      { message: 'the reload must reopen on the turn the reader parked on', timeout: 60_000 },
    ).toBe(PARK_ON_TURN);

    const restored = await restingOn(page);
    expect(Math.abs(restored.relTop - parked.relTop), 'and at the same offset')
      .toBeLessThanOrEqual(1);
    expect(olderReads, 'a position inside the loaded page is never chased').toBe(0);
  });

  test('a position behind the loaded page is chased and lands on its turn', async ({ page }) => {
    const { threadId, messageIds } = seedStepHeavyThread();
    seededThreads.push(threadId);

    let olderReads = 0;
    page.on('request', req => {
      if (isOlderHistoryRead(req.url(), threadId)) olderReads += 1;
    });

    const behindThePage = `anchor:0:${messageIds[0]}`;
    await openThread(page, threadId, behindThePage);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    await expect.poll(() => restingTurn(page, messageIds), {
      message: 'the chase must land the reader on the turn they parked on',
      timeout: 15_000,
    }).toBe(0);
    expect(olderReads, 'the chase is bounded').toBeGreaterThanOrEqual(1);
    expect(olderReads, 'the chase is bounded').toBeLessThanOrEqual(MAX_READING_CHASES);
  });

  test('a position beyond the chase opens inside the loaded pages', async ({ page }) => {
    const turns = Math.ceil((PAGE_SIZE + MAX_READING_CHASES * CHASE_PAGE_SIZE) / EVENTS_PER_TURN) + 20;
    const { threadId, messageIds } = seedStepHeavyThread(turns);
    seededThreads.push(threadId);

    let olderReads = 0;
    page.on('request', req => {
      if (isOlderHistoryRead(req.url(), threadId)) olderReads += 1;
    });

    const beyond = `anchor:0:${messageIds[0]}`;
    await openThread(page, threadId, beyond);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    await page.waitForTimeout(CHASE_WINDOW_MS);

    expect(olderReads, 'the chase stops at its bound').toBeLessThanOrEqual(MAX_READING_CHASES);
    await expect(transcript.locator(`[data-event-id="${messageIds[0]}"]`)).toHaveCount(0);
    expect(await restingTurn(page, messageIds), 'the open rests on a loaded turn').toBeGreaterThan(0);
    const recorded = await page.evaluate((key) => localStorage.getItem(key), scrollKey(threadId));
    expect(recorded, 'giving up must not destroy the reading position').toBe(beyond);
  });

  test('a position the thread shrank under lands at once and holds still', async ({ page }) => {
    const { threadId, messageIds } = seedStepHeavyThread();
    seededThreads.push(threadId);

    await openThread(page, threadId, `anchor:-4000:${messageIds[TURNS - 1]}`);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    await page.waitForTimeout(500);
    const landed = await transcript.evaluate(el => ({
      top: el.scrollTop,
      max: el.scrollHeight - el.clientHeight,
    }));
    expect(landed.max, 'the transcript must scroll, or landing proves nothing').toBeGreaterThan(10);
    expect(landed.top, 'the open must land on the reachable end at once')
      .toBeGreaterThanOrEqual(landed.max - 1);
    const onScreen = await transcript.evaluate((el, id) => {
      const turn = el.querySelector(`[data-event-id="${id}"]`)!.getBoundingClientRect();
      const view = el.getBoundingClientRect();
      return turn.top < view.bottom && turn.bottom > view.top;
    }, messageIds[TURNS - 1]);
    expect(onScreen, 'the turn the reader parked on must be on screen').toBe(true);

    const drift = await transcript.evaluate(async (el) => {
      const start = el.scrollTop;
      let worst = 0;
      const until = performance.now() + 4_000;
      while (performance.now() < until) {
        await new Promise(requestAnimationFrame);
        worst = Math.max(worst, Math.abs(el.scrollTop - start));
      }
      return worst;
    });
    expect(drift, 'nothing may move the transcript after it opens').toBeLessThanOrEqual(1);
  });
});
