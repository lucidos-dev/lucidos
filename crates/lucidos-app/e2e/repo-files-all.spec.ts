import { test, expect, Page } from './fixtures';
import {
  apiRequest, assertHealthy, clickHeaderAction, clickVisibleElement, headerActionOffered,
  hideDashboardRail, navigateToApp, openFilesPanel,
} from './helpers';
import { WORKSPACE, psql, git, getDbPort } from './db-helpers';
import { randomUUID } from 'crypto';
import { execFileSync } from 'child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';
import { join, resolve } from 'path';

async function registerRepo(page: Page, name: string, path: string = WORKSPACE): Promise<string> {
  const resp = await apiRequest(page).post('/api/v1/repositories', {
    data: { name, path, description: 'e2e test repo' },
  });
  expect(resp.ok()).toBeTruthy();
  const body = await resp.json();
  return body.id;
}

async function removeRepo(page: Page, id: string): Promise<void> {
  await apiRequest(page).delete(`/api/v1/repositories/${id}`);
}

function createScratchRepo(): string {
  const dir = mkdtempSync(join(tmpdir(), 'lucidos-e2e-repo-'));
  mkdirSync(join(dir, 'src'));
  writeFileSync(join(dir, 'notes.txt'), 'scratch repo\n');
  writeFileSync(join(dir, 'src', 'main.txt'), 'hello from the scratch repo\n');
  const run = (...args: string[]) => execFileSync('git', args, { cwd: dir });
  run('init', '-q', '-b', 'main');
  run('add', '.');
  run('-c', 'user.name=e2e', '-c', 'user.email=e2e@example.com', 'commit', '-q', '-m', 'seed');
  return dir;
}

test.describe('Repo File Explorer', () => {
  let repoId: string;
  let repoPath: string;
  const repoName = `e2e-repo-${Date.now()}`;

  test.beforeAll(() => {
    repoPath = createScratchRepo();
  });

  test.afterAll(() => {
    if (repoPath) rmSync(repoPath, { recursive: true, force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    repoId = await registerRepo(page, repoName, repoPath);
  });

  test.afterEach(async ({ page }) => {
    if (repoId) await removeRepo(page, repoId);
  });

  test('source switcher appears when repos exist', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);

    const dropdown = page.locator('.files-source-switcher .dropdown-trigger:visible');
    await expect(dropdown).toBeVisible({ timeout: 10_000 });

    await expect(dropdown).toContainText('Workspace');
  });

  test('switching to repo source loads file tree', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);

    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');

    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);

    await page.waitForFunction(() => {
      const items = document.querySelectorAll('.folder-header, .file-item');
      return Array.from(items).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 15_000 });

    const folders = page.locator('.folder-header:visible');
    const folderCount = await folders.count();
    expect(folderCount).toBeGreaterThan(0);
  });

  test('clicking a file opens preview', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);

    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);

    await page.waitForFunction(() => {
      const items = document.querySelectorAll('.file-item');
      return Array.from(items).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 15_000 });

    await page.locator('.file-item').first().click();

    await page.waitForFunction(() => {
      const preview = document.querySelectorAll('.repo-file-content, .file-preview-code');
      return Array.from(preview).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 10_000 });
  });

  test('switching back to workspace restores workspace view', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);

    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);
    await page.waitForSelector('.folder-header:visible', { timeout: 15_000 });

    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', 'Workspace');

    await page.waitForTimeout(500);
    const hasRepoToggle = await page.evaluate(() => {
      const toggles = document.querySelectorAll('.repo-view-toggle');
      return Array.from(toggles).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    });
    expect(hasRepoToggle).toBe(false);
  });
});

test.describe('Repo File Explorer — changes view', () => {
  let repoId: string;
  let branch: string;
  let file: string;
  let changeId: string;
  const suffix = Date.now().toString(36);
  const repoName = `e2e-changes-${suffix}`;
  const changeDescription = 'E2E repo test';

  test.beforeAll(async () => {
    branch = `e2e-test/repo-explorer-${suffix}`;
    file = `e2e-repo-explorer-${suffix}.txt`;
    changeId = randomUUID();

    git(['checkout', '-b', branch, 'main']);
    writeFileSync(resolve(WORKSPACE, file), `repo explorer test ${suffix}`);
    git(['add', '.']);
    git(['commit', '-m', `e2e repo explorer test ${suffix}`]);
    git(['checkout', 'main']);

    const dbPort = getDbPort();
    psql([
      `INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened)`,
      `VALUES ('${changeId}', '${randomUUID()}', '${branch}', '${WORKSPACE}', '${changeDescription}', 1, ARRAY['${file}'], false, true)`,
    ].join(' '));
  });

  test.afterAll(async () => {
    psql(`DELETE FROM changes WHERE id = '${changeId}'`);
    try { git(['branch', '-D', branch]); } catch {  }
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    repoId = await registerRepo(page, repoName);
  });

  test.afterEach(async ({ page }) => {
    if (repoId) await removeRepo(page, repoId);
  });

  test('changes tab shows changed files with badges', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);

    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);

    await page.waitForSelector('.change-selector .dropdown-trigger:visible', { timeout: 10_000 });
    await clickVisibleElement(page, '.change-selector .dropdown-trigger');
    await page.waitForSelector('.change-selector-menu .dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.change-selector-menu .dropdown-option', changeDescription);

    await page.waitForFunction(() => {
      const toggles = document.querySelectorAll('.repo-view-toggle');
      return Array.from(toggles).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 10_000 });

    const changesBtn = page.locator('.repo-view-toggle .files-toolbar-btn.active:visible');
    await expect(changesBtn).toContainText('Changes');

    await page.waitForFunction(() => {
      const badges = document.querySelectorAll('.change-badge');
      return Array.from(badges).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 10_000 });
  });
});

test.describe('Repo File Explorer, side-by-side diff', () => {
  let repoId: string;
  let branch: string;
  let file: string;
  let changeId: string;
  const suffix = Date.now().toString(36);
  const repoName = `e2e-side-by-side-${suffix}`;
  const changeDescription = 'E2E side-by-side diff test';
  const ADDED_LINES = ['first added line', 'second added line', 'third added line'];

  test.beforeAll(async () => {
    branch = `e2e-test/side-by-side-${suffix}`;
    file = `e2e-side-by-side-${suffix}.txt`;
    changeId = randomUUID();

    git(['checkout', '-b', branch, 'main']);
    writeFileSync(resolve(WORKSPACE, file), `${ADDED_LINES.join('\n')}\n`);
    git(['add', '.']);
    git(['commit', '-m', `e2e side-by-side fixture ${suffix}`]);
    git(['checkout', 'main']);

    psql([
      `INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened)`,
      `VALUES ('${changeId}', '${randomUUID()}', '${branch}', '${WORKSPACE}', '${changeDescription}', 1, ARRAY['${file}'], false, true)`,
    ].join(' '));
  });

  test.afterAll(async () => {
    psql(`DELETE FROM changes WHERE id = '${changeId}'`);
    try { git(['branch', '-D', branch]); } catch {  }
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    repoId = await registerRepo(page, repoName);
    await page.addInitScript(() => {
      localStorage.setItem('lucidos-split-ratio', '0.15');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('lucidos-diff-side-by-side', 'false');
    });
    await hideDashboardRail(page);
  });

  test.afterEach(async ({ page }) => {
    if (repoId) await removeRepo(page, repoId);
  });

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
    await expect(page.locator('.diff-view:visible')).toBeVisible({ timeout: 10_000 });
  }

  async function roomForTwoColumns(page: Page): Promise<void> {
    await page.setViewportSize({ width: 1600, height: 900 });
  }

  test('toggles between the unified hunks and two aligned columns', async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name !== 'chromium',
      'a phone has no room for two columns, so the toggle is deliberately absent there',
    );
    await roomForTwoColumns(page);
    await openHunks(page);

    await expect(page.locator('.diff-line:visible').first()).toBeVisible();
    await expect(page.locator('[data-role="side-by-side-diff"]')).toHaveCount(0);

    await clickHeaderAction(page, '.diff-side-by-side-toggle');

    const sideBySide = page.locator('[data-role="side-by-side-diff"]:visible');
    await expect(sideBySide).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.diff-line')).toHaveCount(0);

    const original = page.locator('[data-role="side-by-side-diff-original"]');
    const changed = page.locator('[data-role="side-by-side-diff-changed"]');
    for (const line of ADDED_LINES) {
      await expect(changed).toContainText(line);
      await expect(original).not.toContainText(line);
    }

    const leftRows = await original.locator('.code-line').count();
    const rightRows = await changed.locator('.code-line').count();
    expect(leftRows).toBe(rightRows);
    expect(leftRows).toBeGreaterThan(ADDED_LINES.length);

    await changed.locator('.code-line .line-number').first().click();
    await expect(page.locator('.line-selected')).toHaveCount(0);

    await clickHeaderAction(page, '.diff-side-by-side-toggle');
    await expect(page.locator('[data-role="side-by-side-diff"]')).toHaveCount(0);
    await expect(page.locator('.diff-line:visible').first()).toBeVisible();
  });

  test('does not offer the toggle over the whole merged file', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'chromium', 'the narrow case is covered separately');
    await roomForTwoColumns(page);
    await openHunks(page);
    expect(await headerActionOffered(page, '.diff-side-by-side-toggle')).toBe(true);

    await clickHeaderAction(page, '.diff-whole-file-toggle');
    await expect(page.locator('.repo-file-content:visible')).toBeVisible({ timeout: 10_000 });
    expect(await headerActionOffered(page, '.diff-side-by-side-toggle')).toBe(false);
  });

  test('does not offer the toggle on a phone, where two columns do not fit', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name === 'chromium', 'desktop has the room; this is the narrow case');
    await openHunks(page);

    await expect(page.locator('.diff-line:visible').first()).toBeVisible();
    expect(await headerActionOffered(page, '.diff-side-by-side-toggle')).toBe(false);
  });
});
