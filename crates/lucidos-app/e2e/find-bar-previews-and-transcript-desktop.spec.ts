import { test, expect, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE, seedChatThread } from './db-helpers';
import {
  apiRequest, assertHealthy, clickHeaderAction, disarmFollowSeed, navigateToApp, waitForEventStream,
  waitForScrollSettled,
} from './helpers';

// The find bar over a text preview, an HTML artifact preview and the
// transcript. The unit floor is `store/actions/find-bar.test.ts`,
// `store/actions/transcript-find.test.ts` and the preview bridge tests. This
// is what only a browser can say. Each surface really paints, and the HTML
// preview really answers over its bridge. A step in the transcript reaches a
// turn the window had not drawn.
//
// `-desktop`, because Mod+F and the pane focus it follows are desktop input.
// Plan: `docs/plans/2026-10-07-find-in-previews-and-transcript.md`.

const NOTES_PATH = 'artifacts/e2e-find-notes.md';
const REPORT_PATH = 'artifacts/e2e-find-report.html';
const filler = Array.from({ length: 80 }, (_, i) => `Filler paragraph ${i}.`).join('\n\n');

const status = (page: Page) => page.locator('[data-role="find-status"]:visible');
const findInput = (page: Page) => page.locator('[data-role="find-input"]:visible');
const hostHighlights = (page: Page) =>
  page.evaluate(() => (CSS.highlights.get('lucidos-find') as Highlight | undefined)?.size ?? 0);

async function openFile(page: Page, path: string): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'file', params: { file_path: path } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
}

test.describe('the find bar', () => {
  test.beforeAll(() => {
    mkdirSync(resolve(WORKSPACE, 'data/artifacts'), { recursive: true });
    writeFileSync(resolve(WORKSPACE, 'data', NOTES_PATH),
      `# Notes\n\nA needle near the top.\n\n${filler}\n\nThe last needle, far down.\n`);
    writeFileSync(resolve(WORKSPACE, 'data', REPORT_PATH),
      `<!DOCTYPE html><html><body><h1>Report</h1><p>One needle.</p><p>Two <b>needle</b>s.</p>`
      + `<p style="display:none">A hidden needle.</p><p>Three: needle.</p></body></html>`);
  });

  test.afterAll(() => {
    rmSync(resolve(WORKSPACE, 'data', NOTES_PATH), { force: true });
    rmSync(resolve(WORKSPACE, 'data', REPORT_PATH), { force: true });
  });

  test('finds and steps through a text preview', async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
    await openFile(page, NOTES_PATH);
    await expect(page.locator('.file-preview-frame-body:visible')).toContainText('The last needle', { timeout: 15_000 });

    await clickHeaderAction(page, '.find-btn');
    await findInput(page).fill('needle');
    await expect(status(page)).toHaveText('1 of 2');
    await expect.poll(() => hostHighlights(page)).toBe(2);
    await findInput(page).press('Enter');
    await expect(status(page)).toHaveText('2 of 2');

    await findInput(page).press('Escape');
    await expect(page.locator('[data-role="find-bar"]')).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => CSS.highlights.has('lucidos-find'))).toBe(false);
  });

  test('Mod+F inside an HTML preview finds through its bridge', async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
    await openFile(page, REPORT_PATH);
    await expect(page.locator('iframe[data-role="artifact-preview-frame"]:visible')).toBeVisible({ timeout: 15_000 });
    await expect.poll(() => page.frames().some((f) => f.url() === 'about:srcdoc')).toBe(true);
    const frame = page.frames().find((f) => f.url() === 'about:srcdoc')!;
    await frame.waitForSelector('h1');

    await frame.locator('h1').click();
    await page.keyboard.press('ControlOrMeta+f');
    await expect(findInput(page)).toBeFocused();
    await page.keyboard.type('needle');
    await expect(status(page)).toHaveText('1 of 3');
    await expect.poll(() => frame.evaluate(() => (CSS.highlights.get('lucidos-find') as Highlight | undefined)?.size ?? 0))
      .toBe(3);
    await page.keyboard.press('Enter');
    await expect(status(page)).toHaveText('2 of 3');
  });

  test('finds in the whole transcript, and a step reaches a turn not yet drawn', async ({ page }) => {
    // Forty turns: the transcript draws a tail of about twenty, so turn 2 is
    // not drawn when the thread opens at its end.
    const threadId = seedChatThread({ turns: 40, needles: [2, 38], title: 'Find bar transcript' });
    await page.addInitScript((tid: string) => localStorage.setItem('lucidos-focused-thread', tid), threadId);
    await disarmFollowSeed(page);
    await assertHealthy(page);
    await navigateToApp(page);
    const transcript = page.locator('.thread-view .thread-content:visible');
    await expect(transcript).toContainText('Answer 39.', { timeout: 15_000 });
    await expect(transcript).not.toContainText('Answer 2 mentions');

    // Fold turn 38, so its match is neither drawn nor unfolded.
    const turn38 = page.locator('.chat-exchange', { hasText: 'Question 38' }).first();
    await turn38.locator('[data-role="toggle-collapsed"]').click({ force: true });
    await expect(transcript).not.toContainText('Answer 38 mentions');

    await transcript.click();
    await page.keyboard.press('ControlOrMeta+f');
    await expect(findInput(page)).toBeFocused();
    // The fold and the bar's own roll move the layout. Measure once both rest.
    await waitForScrollSettled(page);
    const before = await transcript.evaluate((el) => el.scrollTop);
    await page.keyboard.type('zebra');
    await expect(status(page)).toHaveText('2 matches');
    expect(await transcript.evaluate((el) => el.scrollTop), 'typing moved the transcript').toBe(before);

    // The first step lands below the reader, on the folded turn, and unfolds it.
    await page.keyboard.press('Enter');
    await expect(status(page)).toHaveText('2 of 2');
    await expect(transcript).toContainText('Answer 38 mentions the zebra.');
    // The next wraps to turn 2, which the window had to draw.
    await page.keyboard.press('Enter');
    await expect(status(page)).toHaveText('1 of 2');
    await expect(transcript).toContainText('Answer 2 mentions the zebra.');
    await expect.poll(() => page.evaluate(() => {
      const current = CSS.highlights.get('lucidos-find-current') as Highlight | undefined;
      const range = current ? [...current][0] as Range : undefined;
      const box = document.querySelector('.thread-view .thread-content')!.getBoundingClientRect();
      const r = range?.getBoundingClientRect();
      return !!r && r.top >= box.top && r.bottom <= box.bottom && range!.toString() === 'zebra';
    })).toBe(true);
  });
});
