/**
 * The find bar holds still on a phone, and every step lands its match where
 * the reader can see it: on the transcript, and sticky at the top of a file.
 *
 * The header is fixed over the transcript, and the bar docks under it. An open
 * bar pins the header (`shouldKeepHeaderVisible`). The field's keyboard and
 * each step's scroll then leave both where they are.
 *
 * A step centres its match in the part of the transcript nothing covers: below
 * the bar, above the prompt.
 *
 * Opening it from the thread menu focuses a text field inside the tap, which
 * is when iOS raises the keyboard. No emulator shows that keyboard, so the
 * spec checks the focus and its timing, not the keyboard itself.
 */
import { mkdirSync, rmSync, writeFileSync } from 'fs';
import { resolve } from 'path';
import { test, expect, type Page } from './fixtures';
import {
  apiRequest, assertHealthy, clickHeaderAction, disableMobileDynamicBars, disarmFollowSeed,
  enableMobileDynamicBars, ensureMobileView, navigateToApp, waitForEventStream,
} from './helpers';
import { WORKSPACE, psql, seedChatThread } from './db-helpers';

test.use({ viewport: { width: 393, height: 852 } });

interface Geometry {
  barTop: number;
  barBottom: number;
  /** Where the header's top edge is: 0 when shown, negative as it slides. */
  headerTop: number;
  headerBottom: number;
  promptTop: number;
  /** The current match's box, or null while there is none. */
  match: { top: number; bottom: number } | null;
  /** The bar's controls a tap on their centre does not reach, by name, each
   *  with what the tap lands on instead. */
  covered: string[];
}

const THREAD_BAR = '.mobile-swipe-pane .thread-view > .find-bar-slot';
const FILE_BAR = '.mobile-swipe-pane .file-preview-frame > .find-bar-slot';
const NOTES_PATH = 'artifacts/e2e-find-phone-notes.md';

async function geometry(page: Page, barSelector = THREAD_BAR): Promise<Geometry | null> {
  return page.evaluate((sel) => {
    const bar = document.querySelector(sel);
    const header = document.querySelector('.app-header');
    const prompt = document.querySelector('.mobile-swipe-pane .prompt-area');
    if (!bar || !header || !prompt) return null;
    const controls = [...bar.querySelectorAll<HTMLElement>('[data-role="find-input"], button')];
    const covered = controls.flatMap((el) => {
      const name = el.getAttribute('aria-label') ?? el.dataset.role ?? el.tagName;
      // A disabled `.icon-btn` takes no pointer events, so every tap passes
      // through it and the check would blame the bar it sits on.
      if ((el as HTMLButtonElement).disabled) return [];
      const r = el.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      if (hit && el.contains(hit)) return [];
      return [`${name} (under ${hit ? `${hit.tagName.toLowerCase()}.${[...hit.classList].join('.')}` : 'nothing'})`];
    });
    const current = CSS.highlights?.get('lucidos-find-current') as Highlight | undefined;
    const selection = window.getSelection();
    const range = current
      ? ([...current][0] as Range | undefined)
      : (selection && selection.rangeCount > 0 ? selection.getRangeAt(0) : undefined);
    const m = range?.getBoundingClientRect();
    const b = bar.getBoundingClientRect();
    const h = header.getBoundingClientRect();
    return {
      barTop: b.top,
      barBottom: b.bottom,
      headerTop: h.top,
      headerBottom: h.bottom,
      promptTop: prompt.getBoundingClientRect().top,
      match: m && m.height > 0 ? { top: m.top, bottom: m.bottom } : null,
      covered,
    };
  }, barSelector);
}

/** Wait until the header rests shown and the bar docks on its bottom edge,
 *  with every control reachable. */
async function expectBarDocked(page: Page, where: string, barSelector = THREAD_BAR): Promise<Geometry> {
  let last: Geometry | null = null;
  await expect.poll(async () => {
    last = await geometry(page, barSelector);
    if (!last) return 'no find bar';
    if (Math.abs(last.headerTop) > 0.5) return `header moved, top at ${last.headerTop}`;
    if (Math.abs(last.barTop - last.headerBottom) > 1) return `bar at ${last.barTop}, header ends at ${last.headerBottom}`;
    return last.covered.length === 0 ? 'docked' : `covered: ${last.covered.join(', ')}`;
  }, { message: where, timeout: 10_000 }).toBe('docked');
  return last!;
}

/** Scroll the transcript down the way a drag does: spaced steps, since one
 *  jump is one event and can land in the app's own navigation window. */
async function dragDown(page: Page) {
  await page.locator('.mobile-swipe-pane .thread-content').evaluate(async (el) => {
    el.scrollTop = (el.scrollHeight - el.clientHeight) / 2;
    for (let i = 0; i < 8; i++) {
      await new Promise((r) => setTimeout(r, 80));
      el.scrollTop += 80;
    }
  });
}

async function openThread(page: Page, needles: number[], title: string, seeded: string[]) {
  const threadId = seedChatThread({ turns: 40, needles, title });
  seeded.push(threadId);
  await disarmFollowSeed(page);
  await page.addInitScript((tid: string) => localStorage.setItem('lucidos-focused-thread', tid), threadId);
  await navigateToApp(page);
  await ensureMobileView(page, 'thread');
  await expect(page.locator('.mobile-swipe-pane .thread-content')).toContainText('Answer 39.', { timeout: 15_000 });
}

test.describe('The transcript find bar on a phone', () => {
  const seeded: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await enableMobileDynamicBars(page);
  });

  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
    if (seeded.length === 0) return;
    const ids = seeded.splice(0).map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  test('docks under the header, which holds still until the bar closes', async ({ page }) => {
    await openThread(page, [3], 'Find bar on a phone', seeded);

    // iOS raises the keyboard only for a focus inside the tap. A window
    // listener runs last in the click's dispatch, so it sees what the tap
    // focused before any render, frame or timer.
    await page.locator('.mobile-thread-title-row .thread-title-menu:visible').click();
    await page.evaluate(() => {
      window.addEventListener('click', () => {
        document.documentElement.dataset.focusedInTap = document.activeElement?.tagName ?? 'none';
      }, { once: true });
    });
    await page.locator('.thread-overflow-menu').getByRole('menuitem', { name: 'Find in thread' }).click();
    await expect(page.locator('html'), 'the tap focused no text field').toHaveAttribute('data-focused-in-tap', 'INPUT');
    const field = page.locator('.mobile-swipe-pane [data-role="find-input"]');
    await expect(field).toBeFocused();

    // A match makes the bar's arrows live, so every control can take a tap.
    await field.fill('zebra');
    await expect(page.locator('.mobile-swipe-pane [data-role="find-status"]')).toHaveText('1 match');
    await expectBarDocked(page, 'while typing, the header stays and the bar docks under it');

    await field.evaluate((el) => (el as HTMLElement).blur());
    await expectBarDocked(page, 'with the field blurred, nothing moves');

    await dragDown(page);
    await expectBarDocked(page, 'the reader\'s own scroll leaves the header pinned while the bar is open');

    // Closed, the bars follow the reader's scroll again.
    await page.locator('.mobile-swipe-pane').getByRole('button', { name: 'Close find' }).click();
    await expect(page.locator('.mobile-swipe-pane .thread-view > .find-bar-slot')).toHaveCount(0);
    await dragDown(page);
    await expect.poll(async () => page.locator('.app-header').evaluate((el) => el.getBoundingClientRect().bottom), {
      message: 'with the bar closed, the header slides away on a scroll down',
    }).toBeLessThanOrEqual(0.5);
  });

  test('every step lands its match clear of the bar and the prompt', async ({ page }) => {
    const needles = [2, 9, 17, 25, 33, 38];
    await openThread(page, needles, 'Find steps on a phone', seeded);
    await page.locator('.mobile-thread-title-row .thread-title-menu:visible').click();
    await page.locator('.thread-overflow-menu').getByRole('menuitem', { name: 'Find in thread' }).click();
    const field = page.locator('.mobile-swipe-pane [data-role="find-input"]');
    await field.fill('zebra');
    const status = page.locator('.mobile-swipe-pane [data-role="find-status"]');
    await expect(status).toHaveText(`${needles.length} matches`);
    const docked = await expectBarDocked(page, 'before the first step');

    // Back through every match from the end, so each one starts off screen
    // or under the chrome, then forward across the wrap.
    for (const key of [...needles.map(() => 'Shift+Enter'), 'Enter', 'Enter']) {
      const before = await status.textContent();
      await field.press(key);
      await expect(status).not.toHaveText(before ?? '');
      await expect.poll(async () => {
        const g = await geometry(page);
        if (!g?.match) return 'no current match';
        if (g.match.top < g.barBottom - 0.5) return `match top ${g.match.top} under the bar ending at ${g.barBottom}`;
        if (g.match.bottom > g.promptTop + 0.5) return `match bottom ${g.match.bottom} under the prompt at ${g.promptTop}`;
        if (Math.abs(g.barTop - docked.barTop) > 1) return `bar moved from ${docked.barTop} to ${g.barTop}`;
        return 'clear';
      }, { message: `after ${key}, status ${await status.textContent()}`, timeout: 10_000 }).toBe('clear');
    }
  });

  test('a file\'s find bar sticks under the pinned header, and no step lands under it', async ({ page }) => {
    mkdirSync(resolve(WORKSPACE, 'data/artifacts'), { recursive: true });
    const filler = Array.from({ length: 120 }, (_, i) => `Filler paragraph ${i}.`).join('\n\n');
    writeFileSync(resolve(WORKSPACE, 'data', NOTES_PATH), `# Notes\n\nA zebra near the top.\n\n${filler}\n\nThe last zebra.\n`);
    try {
      await navigateToApp(page);
      await waitForEventStream(page);
      const res = await apiRequest(page).post('/api/v1/ui/navigate', {
        headers: { 'content-type': 'application/json' },
        data: { target: 'file', params: { file_path: NOTES_PATH } },
      });
      expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
      await ensureMobileView(page, 'content');
      const body = page.locator('.mobile-swipe-pane .file-preview-frame-body');
      await expect(body).toContainText('The last zebra', { timeout: 15_000 });

      await clickHeaderAction(page, '.find-btn');
      const field = page.locator('.mobile-swipe-pane [data-role="find-input"]');
      await expect(field).toBeFocused();
      const status = page.locator('.mobile-swipe-pane [data-role="find-status"]');
      await field.fill('z');
      // A literal on purpose: it pins the hint the reader sees, which names
      // MIN_FIND_QUERY_CHARS from a store module Playwright cannot import.
      await expect(status, 'one character searches nothing yet').toHaveText('Type 2+ characters');
      await field.fill('zebra');
      await expect(status).toHaveText('1 of 2');
      await expectBarDocked(page, 'while typing, the bar docks under the header', FILE_BAR);

      /** The current match sits below the bar and on screen, with the bar docked. */
      const expectMatchClear = (where: string) => expect.poll(async () => {
        const g = await geometry(page, FILE_BAR);
        if (!g?.match) return 'no current match';
        if (g.match.top < g.barBottom - 0.5) return `match top ${g.match.top} under the bar ending at ${g.barBottom}`;
        const vh = await page.evaluate(() => window.innerHeight);
        if (g.match.bottom > vh + 0.5) return `match bottom ${g.match.bottom} below the screen at ${vh}`;
        if (Math.abs(g.barTop - g.headerBottom) > 1) return `bar at ${g.barTop}, header ends at ${g.headerBottom}`;
        return 'clear';
      }, { message: where, timeout: 10_000 }).toBe('clear');

      // A step to the far match scrolls the file a long way under the bar.
      await field.press('Enter');
      await expect(status).toHaveText('2 of 2');
      await expectMatchClear('the far match lands clear of the bar');

      // The first match parked just under the bar still counts as hidden, so
      // the wrap back to it scrolls it clear.
      await page.evaluate((sel) => {
        const bar = document.querySelector(sel)!.getBoundingClientRect();
        const first = [...document.querySelectorAll('.mobile-swipe-pane .file-preview-frame-body p')]
          .find((p) => p.textContent?.includes('A zebra near the top'))!;
        const scroller = document.querySelector('.mobile-swipe-pane .content-pane-body')!;
        scroller.scrollTop += first.getBoundingClientRect().top - (bar.top + 2);
      }, FILE_BAR);
      await field.press('Enter');
      await expect(status).toHaveText('1 of 2');
      await expectMatchClear('a match under the bar is scrolled clear of it');

      await page.locator('.mobile-swipe-pane .content-pane-body').evaluate(async (el) => {
        el.scrollTop = (el.scrollHeight - el.clientHeight) / 2;
        for (let i = 0; i < 8; i++) {
          await new Promise((r) => setTimeout(r, 80));
          el.scrollTop += 80;
        }
      });
      await expectBarDocked(page, 'scrolled mid-file, the header stays pinned and the bar sticks under it', FILE_BAR);
    } finally {
      rmSync(resolve(WORKSPACE, 'data', NOTES_PATH), { force: true });
    }
  });
});
