import { test, expect, Page } from './fixtures';
import { navigateToApp, assertHealthy, openFilesPanel, clickVisibleElement, clickHeaderAction } from './helpers';
import { WORKSPACE, psql, git, getDbPort } from './db-helpers';
import { randomUUID } from 'crypto';
import { writeFileSync } from 'fs';
import { resolve } from 'path';

/** Register the e2e workspace as a repository via API. */
async function registerRepo(page: Page, name: string): Promise<string> {
  const resp = await page.request.post('/api/v1/repositories', {
    data: { name, path: WORKSPACE, description: 'e2e test repo' },
  });
  expect(resp.ok()).toBeTruthy();
  const body = await resp.json();
  return body.id;
}

/** Remove a repository via API. */
async function removeRepo(page: Page, id: string): Promise<void> {
  await page.request.delete(`/api/v1/repositories/${id}`);
}

test.describe('Repo File Explorer', () => {
  let repoId: string;
  const repoName = `e2e-repo-${Date.now()}`;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    repoId = await registerRepo(page, repoName);
  });

  test.afterEach(async ({ page }) => {
    if (repoId) await removeRepo(page, repoId);
  });

  test('source switcher appears when repos exist', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);

    // The source switcher dropdown should be visible
    const dropdown = page.locator('.files-source-switcher .dropdown-trigger:visible');
    await expect(dropdown).toBeVisible({ timeout: 10_000 });

    // Default selection should be "Workspace"
    await expect(dropdown).toContainText('Workspace');
  });

  test('switching to repo source loads file tree', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);

    // Open source switcher dropdown
    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');

    // Click the repo option
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);

    // Wait for file tree to load — folder-tree or file-item should appear
    await page.waitForFunction(() => {
      const items = document.querySelectorAll('.folder-header, .file-item');
      return Array.from(items).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 15_000 });

    // Verify folder tree rendered
    const folders = page.locator('.folder-header:visible');
    const folderCount = await folders.count();
    expect(folderCount).toBeGreaterThan(0);
  });

  test('clicking a file opens preview', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);

    // Switch to repo
    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);

    // Wait for files to load
    await page.waitForFunction(() => {
      const items = document.querySelectorAll('.file-item');
      return Array.from(items).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 15_000 });

    // Click a file item
    await clickVisibleElement(page, '.file-item');

    // File preview should appear with code content
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

    // Switch to repo
    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);
    await page.waitForSelector('.folder-header:visible', { timeout: 15_000 });

    // Switch back to workspace
    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', 'Workspace');

    // Should no longer show repo-specific UI (view toggle)
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
    // Create a branch with a change
    branch = `e2e-test/repo-explorer-${suffix}`;
    file = `e2e-repo-explorer-${suffix}.txt`;
    changeId = randomUUID();

    git(['checkout', '-b', branch, 'main']);
    writeFileSync(resolve(WORKSPACE, file), `repo explorer test ${suffix}`);
    git(['add', '.']);
    git(['commit', '-m', `e2e repo explorer test ${suffix}`]);
    git(['checkout', 'main']);

    // Insert pending change in DB
    const dbPort = getDbPort();
    psql([
      `INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened)`,
      `VALUES ('${changeId}', '${randomUUID()}', '${branch}', '${WORKSPACE}', '${changeDescription}', 1, ARRAY['${file}'], false, true)`,
    ].join(' '));
  });

  test.afterAll(async () => {
    // Cleanup
    psql(`DELETE FROM changes WHERE id = '${changeId}'`);
    try { git(['branch', '-D', branch]); } catch { /* */ }
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

    // Switch to the test's registered repo by name
    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);

    // Repo defaults to All Files view; explicitly pick the pending change
    // via the ChangeSelector to switch to the Changes view.
    await page.waitForSelector('.change-selector .dropdown-trigger:visible', { timeout: 10_000 });
    await clickVisibleElement(page, '.change-selector .dropdown-trigger');
    await page.waitForSelector('.change-selector-menu .dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.change-selector-menu .dropdown-option', changeDescription);

    // After selecting a change, the view toggle appears
    await page.waitForFunction(() => {
      const toggles = document.querySelectorAll('.repo-view-toggle');
      return Array.from(toggles).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 10_000 });

    // The Changes tab should be active when a pending change is selected
    const changesBtn = page.locator('.repo-view-toggle .files-toolbar-btn.active:visible');
    await expect(changesBtn).toContainText('Changes');

    // Changed files should have change badges
    await page.waitForFunction(() => {
      const badges = document.querySelectorAll('.change-badge');
      return Array.from(badges).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 10_000 });
  });
});

/** The side-by-side rendering of a diff: the original on the left, the changed
 *  file on the right, aligned row for row. A mode of the existing diff surface,
 *  so these drive it through the same header the whole-file toggle lives in. */
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
    try { git(['branch', '-D', branch]); } catch { /* */ }
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    repoId = await registerRepo(page, repoName);
    // Two columns are offered only where they fit, so the diff pane has to
    // clear the threshold. Narrowing the Conversation side gives the content
    // pane the width without depending on the project's viewport. The side-by-side
    // preference is pinned off so each test starts from the unified view.
    await page.addInitScript(() => {
      localStorage.setItem('lucidos-split-ratio', '0.15');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('lucidos-diff-side-by-side', 'false');
    });
  });

  test.afterEach(async ({ page }) => {
    if (repoId) await removeRepo(page, repoId);
  });

  /** Open the seeded change's one file, on its unified hunks.
   *
   *  An added file defaults to the whole merged file (its diff is all
   *  additions), so the whole-file toggle is flipped to reach the hunks. */
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

  test('toggles between the unified hunks and two aligned columns', async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name !== 'chromium',
      'a phone has no room for two columns, so the toggle is deliberately absent there',
    );
    await openHunks(page);

    // Unified to begin with: one column carrying both files' line numbers.
    await expect(page.locator('.diff-line:visible').first()).toBeVisible();
    await expect(page.locator('[data-role="side-by-side-diff"]')).toHaveCount(0);

    await clickHeaderAction(page, '.diff-side-by-side-toggle');

    const sideBySide = page.locator('[data-role="side-by-side-diff"]:visible');
    await expect(sideBySide).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.diff-line')).toHaveCount(0);

    // Every line of this change is an addition, so the changed side carries all
    // of them and the original side is filler across from each one.
    const original = page.locator('[data-role="side-by-side-diff-original"]');
    const changed = page.locator('[data-role="side-by-side-diff-changed"]');
    for (const line of ADDED_LINES) {
      await expect(changed).toContainText(line);
      await expect(original).not.toContainText(line);
    }

    // Aligned row for row: the columns always hold the same number of rows,
    // fillers included, or the two sides drift apart down the file.
    const leftRows = await original.locator('.code-line').count();
    const rightRows = await changed.locator('.code-line').count();
    expect(leftRows).toBe(rightRows);
    expect(leftRows).toBeGreaterThan(ADDED_LINES.length);

    // A column is not the file's own line numbering (the left is the old
    // file's, the right the new one's), so clicking a number selects nothing.
    await changed.locator('.code-line .line-number').first().click();
    await expect(page.locator('.line-selected')).toHaveCount(0);

    // And back.
    await clickHeaderAction(page, '.diff-side-by-side-toggle');
    await expect(page.locator('[data-role="side-by-side-diff"]')).toHaveCount(0);
    await expect(page.locator('.diff-line:visible').first()).toBeVisible();
  });

  // A control that is present but does nothing is a lie about what the surface
  // can do: side by side is a rendering of the HUNKS.
  test('does not offer the toggle over the whole merged file', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'chromium', 'the narrow case is covered separately');
    await openHunks(page);
    await expect(page.locator('.diff-side-by-side-toggle:visible')).toBeVisible();

    // Back to the whole merged file.
    await clickHeaderAction(page, '.diff-whole-file-toggle');
    await expect(page.locator('.repo-file-content:visible')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.diff-side-by-side-toggle')).toHaveCount(0);
  });

  test('does not offer the toggle on a phone, where two columns do not fit', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name === 'chromium', 'desktop has the room; this is the narrow case');
    await openHunks(page);

    await expect(page.locator('.diff-line:visible').first()).toBeVisible();
    await expect(page.locator('.diff-side-by-side-toggle')).toHaveCount(0);
  });
});
