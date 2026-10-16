import { test, expect, Page } from './fixtures';
import {
  apiRequest, assertHealthy, clickHeaderAction, clickVisibleElement, disableMobileDynamicBars,
  enableMobileDynamicBars, navigateToApp, openFilesPanel,
} from './helpers';
import { WORKSPACE, psql, git } from './db-helpers';
import { randomUUID } from 'crypto';
import { writeFileSync } from 'fs';
import { resolve } from 'path';

/** A diff on a phone scrolls the content pane itself, so the header hides.
 *
 *  The mobile header watches one scroller per pane, `.content-pane-body`, and
 *  that scroller reserves the header's height in a spacer. The diff used to be
 *  a pane-height box with its own inner scroller under that spacer. A drag on
 *  the diff then scrolled only the inner box and the header never moved. A drag
 *  on the path row scrolled the pane by one header height and stopped dead. */
test.describe('Diff preview scrolling on a phone', () => {
  let repoId: string;
  let branch: string;
  let file: string;
  let changeId: string;
  const suffix = Date.now().toString(36);
  const repoName = `e2e-diff-scroll-${suffix}`;
  const changeDescription = 'E2E diff scroll test';
  const ADDED_LINES = Array.from({ length: 200 }, (_, i) => `added line ${i + 1}`);

  test.beforeAll(async () => {
    branch = `e2e-test/diff-scroll-${suffix}`;
    file = `e2e-diff-scroll-${suffix}.txt`;
    changeId = randomUUID();

    git(['checkout', '-b', branch, 'main']);
    writeFileSync(resolve(WORKSPACE, file), `${ADDED_LINES.join('\n')}\n`);
    git(['add', '.']);
    git(['commit', '-m', `e2e diff scroll fixture ${suffix}`]);
    git(['checkout', 'main']);

    psql([
      `INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened)`,
      `VALUES ('${changeId}', '${randomUUID()}', '${branch}', '${WORKSPACE}', '${changeDescription}', 1, ARRAY['${file}'], false, true)`,
    ].join(' '));
  });

  test.afterAll(async () => {
    psql(`DELETE FROM changes WHERE id = '${changeId}'`);
    try { git(['branch', '-D', branch]); } catch { /* */ }
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    const resp = await apiRequest(page).post('/api/v1/repositories', {
      data: { name: repoName, path: WORKSPACE, description: 'e2e test repo' },
    });
    expect(resp.ok()).toBeTruthy();
    repoId = (await resp.json()).id;
    // The header only hides with dynamic bars on; pinned bars never move.
    await enableMobileDynamicBars(page);
  });

  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
    if (repoId) await apiRequest(page).delete(`/api/v1/repositories/${repoId}`);
  });

  /** Open the seeded change's file on its hunks. An added file opens on the
   *  whole merged file, so the toggle is flipped to reach the diff. */
  async function openHunks(page: Page): Promise<void> {
    await navigateToApp(page);
    await openFilesPanel(page);

    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);

    await page.waitForSelector('.change-selector .dropdown-trigger:visible', { timeout: 10_000 });
    await clickVisibleElement(page, '.change-selector .dropdown-trigger');
    await page.waitForSelector('.change-selector-menu .dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.change-selector-menu .dropdown-option', changeDescription);

    await page.waitForSelector('.file-item:visible', { timeout: 15_000 });
    await clickVisibleElement(page, '.file-item', file);

    await clickHeaderAction(page, '.diff-whole-file-toggle');
    await expect(page.locator('.diff-line:visible').last()).toBeAttached({ timeout: 10_000 });
  }

  /** Class names of every box inside the pane that scrolls vertically on its
   *  own. Empty means each is as tall as its content, so the pane scrolls. */
  async function innerScrollers(page: Page): Promise<string[]> {
    return page.evaluate(() => {
      const body = document.querySelector('.mobile-swipe-pane .content-pane-body');
      if (!body) return ['no pane body'];
      return Array.from(body.querySelectorAll<HTMLElement>('*'))
        .filter((el) => el.scrollHeight > el.clientHeight + 1 && ['auto', 'scroll'].includes(getComputedStyle(el).overflowY))
        .map((el) => el.className);
    });
  }

  test('the whole file scrolls the pane as well', async ({ page }) => {
    await openHunks(page);
    await clickHeaderAction(page, '.diff-whole-file-toggle');
    await expect(page.locator('.repo-file-content:visible')).toBeVisible({ timeout: 10_000 });
    expect(await innerScrollers(page)).toEqual([]);
  });

  test('the diff scrolls the pane, and the header hides', async ({ page }) => {
    await openHunks(page);
    expect(await innerScrollers(page)).toEqual([]);

    // Scroll the pane the way a drag does, in steps spaced past the app's own
    // navigation-scroll window (see thread-title.spec.ts for why).
    const scrolled = await page.evaluate(async () => {
      const body = document.querySelector('.mobile-swipe-pane .content-pane-body');
      if (!body) return 0;
      for (let i = 0; i < 20; i++) {
        const before = body.scrollTop;
        body.scrollTop = before + 120;
        if (body.scrollTop === before) break;
        await new Promise((r) => setTimeout(r, 80));
      }
      return body.scrollTop;
    });
    // Far past one header height: the pane is no longer stuck at the spacer.
    expect(scrolled).toBeGreaterThan(1000);

    await page.waitForFunction(() => {
      const header = document.querySelector('.app-header');
      return !!header && header.getBoundingClientRect().bottom <= 0;
    }, undefined, { timeout: 5_000 });
  });
});
