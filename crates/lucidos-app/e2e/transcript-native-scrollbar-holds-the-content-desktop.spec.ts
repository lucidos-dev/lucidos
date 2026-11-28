import { test, expect, Page } from './fixtures';
import type { Locator, Route } from '@playwright/test';
import { navigateToApp, assertHealthy, disarmFollowSeed, waitForScrollSettled, followToggleState } from './helpers';
import { psql, seedStepHeavyThread } from './db-helpers';


test.use({
  launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] },
  serviceWorkers: 'block',
});

const TURNS = 60;
const STEPS_PER_TURN = 8;

const MOVE_PX = 60;
const REST_MS = 500;
const MAX_CYCLES = 30;
const STILL_TOLERANCE_PX = 2;

async function openThread(page: Page, threadId: string, { armed = false } = {}): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  if (!armed) await disarmFollowSeed(page);
  await navigateToApp(page);
}

async function gutterX(transcript: Locator): Promise<number> {
  const bar = await transcript.evaluate((el: HTMLElement) => {
    const left = el.getBoundingClientRect().left + el.clientLeft + el.clientWidth;
    const width = el.offsetWidth - el.clientWidth - el.clientLeft * 2;
    return { x: left + width / 2, width };
  });
  expect(bar.width, 'the transcript reserves a native scrollbar gutter').toBeGreaterThan(0);
  return bar.x;
}

interface Frame {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  top: number;
}

async function frameOf(transcript: Locator): Promise<Frame> {
  return transcript.evaluate((el) => ({
    scrollTop: el.scrollTop,
    scrollHeight: el.scrollHeight,
    clientHeight: el.clientHeight,
    top: el.getBoundingClientRect().top + el.clientTop,
  }));
}

function thumbMiddleY(f: Frame): number {
  const length = (f.clientHeight * f.clientHeight) / f.scrollHeight;
  const range = f.scrollHeight - f.clientHeight;
  const offset = range > 0 ? ((f.clientHeight - length) * f.scrollTop) / range : 0;
  return f.top + offset + length / 2;
}

async function sampleStepInView(transcript: Locator): Promise<() => Promise<Array<number | null>>> {
  const step = await transcript.evaluate((el) => {
    const w = window as unknown as { __stepSamples: Array<number | null>; __sampling: boolean };
    const steps = () => {
      const found: Array<{ text: string; box: DOMRect }> = [];
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        const text = node.textContent?.trim() ?? '';
        if (!/^Run echo \d+\.\d+$/.test(text)) continue;
        const range = document.createRange();
        range.selectNodeContents(node);
        found.push({ text, box: range.getBoundingClientRect() });
      }
      return found;
    };
    const edge = el.getBoundingClientRect();
    const inView = steps().find(({ box }) => box.top >= edge.top && box.bottom <= edge.bottom);
    if (!inView) return '';
    w.__stepSamples = [];
    w.__sampling = true;
    const tick = () => {
      const now = steps().find(({ text }) => text === inView.text);
      w.__stepSamples.push(now ? now.box.top - el.getBoundingClientRect().top : null);
      if (w.__sampling) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    return inView.text;
  });
  expect(step, 'a step row is in view').not.toBe('');
  return () => transcript.evaluate(() => {
    const w = window as unknown as { __stepSamples: Array<number | null>; __sampling: boolean };
    w.__sampling = false;
    return w.__stepSamples;
  });
}

async function settleFrames(transcript: Locator): Promise<void> {
  await transcript.evaluate(() => new Promise<void>((done) => {
    requestAnimationFrame(() => requestAnimationFrame(() => done()));
  }));
}

async function firstTurnDrawn(transcript: Locator): Promise<boolean> {
  return transcript.evaluate((el) =>
    Array.from(el.querySelectorAll<HTMLElement>('.chat-exchange')).some(turn => /\bturn 0\b/.test(turn.innerText)));
}

test.describe('The desktop transcript on its native scrollbar', () => {
  let threadId = '';

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    ({ threadId } = seedStepHeavyThread({
      turns: TURNS,
      stepsPerTurn: STEPS_PER_TURN,
      title: 'E2E native scrollbar holds the content',
    }));
  });

  test.afterEach(() => {
    psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`);
  });

  test('a thumb drag to the first turn never moves the content under the reader', async ({ page }) => {
    test.setTimeout(240_000);

    const heldPages: Route[] = [];
    let pagesLetThroughWhileHeld = 0;
    await page.route(/\/threads\/[^/?]+\/events\?[^#]*before_created=/, (route) => {
      heldPages.push(route);
    });

    await openThread(page, threadId);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    const x = await gutterX(transcript);

    await transcript.evaluate((el) => { el.scrollTop = el.scrollHeight; });
    await waitForScrollSettled(page);

    const holdsStill = async (label: string, held: boolean, act: () => Promise<void> = async () => {}) => {
      await settleFrames(transcript);
      const stop = await sampleStepInView(transcript);
      await act();
      await settleFrames(transcript);
      await page.waitForTimeout(100);
      const released = heldPages.splice(0);
      for (const route of released) await route.continue();
      if (held) pagesLetThroughWhileHeld += released.length;
      await page.waitForTimeout(REST_MS);
      const samples = await stop();
      expect(samples.length, 'frames were sampled').toBeGreaterThan(1);
      expect(samples.every(s => s !== null), `${label}: the step in view stayed drawn`).toBe(true);
      const offsets = samples as number[];
      const drift = Math.max(...offsets) - Math.min(...offsets);
      expect(drift, `${label}: the content moved under the reader`).toBeLessThanOrEqual(STILL_TOLERANCE_PX);
    };

    let releasesThatDrew = 0;
    for (let cycle = 0; cycle < MAX_CYCLES && !(await firstTurnDrawn(transcript)); cycle++) {
      let y = thumbMiddleY(await frameOf(transcript));
      await page.mouse.move(x, y);
      await page.mouse.down();
      while (y > 1) {
        y = Math.max(1, y - MOVE_PX);
        await page.mouse.move(x, y, { steps: 3 });
        await holdsStill(`cycle ${cycle}, held`, true);
      }
      const drawnBefore = await transcript.locator('.chat-exchange').count();
      await holdsStill(`cycle ${cycle}, released`, false, () => page.mouse.up());
      if (await transcript.locator('.chat-exchange').count() > drawnBefore) releasesThatDrew++;
    }

    expect(await firstTurnDrawn(transcript), 'the drag reached the first turn').toBe(true);
    expect(releasesThatDrew, 'a release drew older turns').toBeGreaterThan(0);
    expect(pagesLetThroughWhileHeld, 'a page reached the app while the thumb was held').toBeGreaterThan(0);
  });

  test('a thumb drag up off the live edge leaves an armed reader where they dragged', async ({ page }) => {
    await openThread(page, threadId, { armed: true });
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();
    await expect.poll(() => followToggleState(page)).toBe('true');
    await waitForScrollSettled(page);
    const x = await gutterX(transcript);

    const start = await frameOf(transcript);
    await page.mouse.move(x, thumbMiddleY(start));
    await page.mouse.down();
    await page.mouse.move(x, start.top + start.clientHeight - 1, { steps: 5 });
    await expect.poll(async () => {
      const f = await frameOf(transcript);
      return f.scrollHeight - f.clientHeight - f.scrollTop;
    }, { message: 'the drag reached the live edge' }).toBeLessThanOrEqual(2);
    await page.waitForTimeout(1500);

    let y = thumbMiddleY(await frameOf(transcript));
    for (let move = 0; move < 4; move++) {
      y -= MOVE_PX;
      await page.mouse.move(x, y, { steps: 3 });
      await page.waitForTimeout(300);
    }

    const tops = await transcript.evaluate((el) => new Promise<number[]>((done) => {
      const seen: number[] = [];
      const tick = () => {
        seen.push(el.scrollTop);
        if (seen.length < 30) requestAnimationFrame(tick);
        else done(seen);
      };
      requestAnimationFrame(tick);
    }));
    const held = await frameOf(transcript);
    await page.mouse.up();

    const drift = Math.max(...tops) - Math.min(...tops);
    expect(drift, 'the content moved while the thumb was held still').toBeLessThanOrEqual(STILL_TOLERANCE_PX);
    expect(held.scrollHeight - held.clientHeight - held.scrollTop, 'the drag took the reader off the live edge')
      .toBeGreaterThan(MOVE_PX);
  });
});
