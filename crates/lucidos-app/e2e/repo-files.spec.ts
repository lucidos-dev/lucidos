import { test, expect, Page } from '@playwright/test';
import { navigateToApp, assertHealthy, openFilesPanel, clickVisibleElement } from './helpers';
import { WORKSPACE, psql, git, getDbPort } from './db-helpers';
import { randomUUID } from 'crypto';
import { writeFileSync } from 'fs';
import { resolve } from 'path';

/** Register the e2e workspace as a repository via API. */
async function registerRepo(page: Page, name: string): Promise<string> {
  const resp = await page.request.post('/api/repositories', {
    data: { name, path: WORKSPACE, description: 'e2e test repo' },
  });
  expect(resp.ok()).toBeTruthy();
  const body = await resp.json();
  return body.id;
}

/** Remove a repository via API. */
async function removeRepo(page: Page, id: string): Promise<void> {
  await page.request.delete(`/api/repositories/${id}`);
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

  test('expand/collapse all folders', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);

    // Switch to repo
    await clickVisibleElement(page, '.files-source-switcher .dropdown-trigger');
    await page.waitForSelector('.dropdown-option:visible', { timeout: 5_000 });
    await clickVisibleElement(page, '.dropdown-option', repoName);

    // Wait for tree
    await page.waitForSelector('.folder-header:visible', { timeout: 15_000 });

    // Click "Expand All"
    await clickVisibleElement(page, '.files-toolbar-btn', 'Expand All');
    await page.waitForTimeout(500);

    // Count visible folders after expand
    const expandedCount = await page.evaluate(() => {
      const headers = document.querySelectorAll('.folder-header');
      return Array.from(headers).filter(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      }).length;
    });
    expect(expandedCount).toBeGreaterThan(2);

    // Click "Collapse All"
    await clickVisibleElement(page, '.files-toolbar-btn', 'Collapse All');
    await page.waitForTimeout(500);

    // After collapse, only top-level folders visible
    const collapsedCount = await page.evaluate(() => {
      const headers = document.querySelectorAll('.folder-header');
      return Array.from(headers).filter(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      }).length;
    });
    expect(collapsedCount).toBeLessThan(expandedCount);
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
