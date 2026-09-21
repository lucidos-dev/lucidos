/**
 * The previewed file can be taken out of the shell.
 *
 * The unit tests pin WHAT the control points at, per platform. What they cannot
 * see is whether the header ever asks for it. So this covers the wiring, in
 * three claims. It is offered over a real preview. It is a real anchor at the
 * file's own URL. It leaves while the inline editor owns the header.
 *
 * Placement is deliberately not asserted. An HTML artifact's header already
 * carries three context actions, so `alwaysCollapseFrom` folds them all behind
 * `⋯`. Which placement it takes is the layout's business, not this test's.
 */
import { test, expect, type Locator, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import {
  apiRequest, assertHealthy, clickHeaderAction, clickVisibleElement, headerActionOffered,
  navigateToApp, waitForEventStream,
} from './helpers';

const SAMPLE_PATH = 'artifacts/e2e-popout-report.html';
const SAMPLE_FILE = resolve(WORKSPACE, 'data', SAMPLE_PATH);
const POPOUT = '.file-open-in-tab';

/** The control wherever collapse put it, opening the `⋯` menu when that is
 *  where it went. Call only once `headerActionOffered` has said it is there. */
async function popoutControl(page: Page): Promise<Locator> {
  const inHeader = page.locator(`${POPOUT}:visible`);
  if (await inHeader.count() > 0) return inHeader.first();
  expect(await clickVisibleElement(page, '.content-header-more')).toBe(true);
  const row = page.locator(`.thread-overflow-item${POPOUT}`);
  await expect(row).toBeVisible({ timeout: 10_000 });
  return row;
}

async function openArtifact(page: Page): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'file', params: { file_path: SAMPLE_PATH } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
  await expect(page.locator('.file-preview-inline:visible iframe').first())
    .toBeVisible({ timeout: 15_000 });
}

test.describe('the previewed file', () => {
  test.beforeAll(() => {
    mkdirSync(resolve(WORKSPACE, 'data/artifacts'), { recursive: true });
    writeFileSync(SAMPLE_FILE, '<!DOCTYPE html>\n<html><body><h1>report</h1></body></html>\n');
  });

  test.afterAll(() => {
    rmSync(SAMPLE_FILE, { force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    // The navigate is delivered over SSE, so the stream has to be up first.
    await waitForEventStream(page);
  });

  test('can be opened outside the shell, at its own URL', async ({ page }) => {
    await openArtifact(page);

    expect(await headerActionOffered(page, POPOUT)).toBe(true);
    const control = await popoutControl(page);
    // A real anchor is what keeps cmd-click, middle-click and "copy link
    // address" working. A button that opens a tab looks the same and does not.
    await expect(control).toHaveAttribute('href', `/data/${SAMPLE_PATH}`);
    await expect(control).toHaveAttribute('target', '_blank');
  });

  test('offers nothing to open while the inline editor holds the header', async ({ page }) => {
    await openArtifact(page);
    await clickHeaderAction(page, '.file-edit-btn');
    await expect(page.locator('.file-editor-textarea:visible')).toBeVisible({ timeout: 10_000 });

    // Editing replaces the whole context cluster. A control that opened the
    // saved copy from under an unsaved draft would lie about what it shows.
    //
    // A short timeout on purpose: neither placement exists here, so the helper's
    // settle wait can only run out, and the editor being on screen already
    // proves the header has re-rendered.
    expect(await headerActionOffered(page, POPOUT, 3_000)).toBe(false);
  });
});
