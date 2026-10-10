import { test, expect, Page } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp } from './helpers';
import { WORKSPACE } from './db-helpers';
import { execFileSync } from 'child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';
import { join, resolve } from 'path';

/** A tester's reported gap: a relative link such as `[x](../../other.md)`
 *  inside a rendered markdown preview must open its target in Lucidos' own
 *  preview. Covers a workspace artifact AND a registered-repository file. */

async function openByPath(page: Page, filePath: string): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'file', params: { file_path: filePath } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
  await expect(page.locator('.file-preview-path:visible')).toBeVisible({ timeout: 15_000 });
}

const WS_ROOT = 'artifacts/e2e-relative-links';
const WS_START = `${WS_ROOT}/sub/deep/start.md`;
const WS_GUIDE = `${WS_ROOT}/guide.md`;

test.describe('a relative link in a workspace markdown preview', () => {
  test.beforeAll(() => {
    mkdirSync(resolve(WORKSPACE, 'data', `${WS_ROOT}/sub/deep`), { recursive: true });
    // Leading prose, not a bare link: a link starting flush at the content
    // edge sits under the mobile edge-swipe zone and is unclickable there.
    writeFileSync(resolve(WORKSPACE, 'data', WS_START), 'See the [guide](../../guide.md) for more.\n');
    writeFileSync(resolve(WORKSPACE, 'data', WS_GUIDE), '# Guide\n\nYou made it to the guide.\n');
  });

  test.afterAll(() => {
    rmSync(resolve(WORKSPACE, 'data', WS_ROOT), { recursive: true, force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a deep ../../ link opens its target in the file preview', async ({ page }) => {
    await navigateToApp(page);
    await openByPath(page, WS_START);

    await page.locator('.file-preview-inline:visible .markdown-content a', { hasText: 'guide' }).click();

    await expect(page.locator('.file-preview-path-name:visible')).toHaveText('guide.md');
    await expect(page.locator('.file-preview-inline:visible .markdown-content'))
      .toContainText('You made it to the guide.');
  });
});

/** A committed scratch repo with two markdown files, one deep under the other,
 *  linking back up with `../../`. */
function createScratchRepo(): string {
  const dir = mkdtempSync(join(tmpdir(), 'lucidos-e2e-repo-links-'));
  mkdirSync(join(dir, 'docs', 'sub', 'deep'), { recursive: true });
  // Leading prose, not a bare link: see the comment on the workspace fixture.
  writeFileSync(join(dir, 'docs', 'sub', 'deep', 'start.md'), 'See the [guide](../../guide.md) for more.\n');
  writeFileSync(join(dir, 'docs', 'guide.md'), '# Guide\n\nYou made it from the repo.\n');
  const run = (...args: string[]) => execFileSync('git', args, { cwd: dir });
  run('init', '-q', '-b', 'main');
  run('add', '.');
  run('-c', 'user.name=e2e', '-c', 'user.email=e2e@example.com', 'commit', '-q', '-m', 'seed');
  return dir;
}

async function registerRepo(page: Page, name: string, path: string): Promise<string> {
  const resp = await apiRequest(page).post('/api/v1/repositories', {
    data: { name, path, description: 'e2e test repo' },
  });
  expect(resp.ok()).toBeTruthy();
  return (await resp.json()).id;
}

async function removeRepo(page: Page, id: string): Promise<void> {
  await apiRequest(page).delete(`/api/v1/repositories/${id}`);
}

test.describe('a relative link in a repo markdown preview', () => {
  let repoId: string;
  let repoPath: string;
  const repoName = `e2e-repo-links-${Date.now()}`;

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

  test('a deep ../../ link opens its target inside the same checkout', async ({ page }) => {
    await navigateToApp(page);
    await openByPath(page, `repo:${repoId}:file:docs/sub/deep/start.md`);

    await page.locator('.repo-file-rendered .markdown-content a', { hasText: 'guide' }).click();

    await expect(page.locator('.file-preview-path-name:visible')).toHaveText('guide.md');
    await expect(page.locator('.repo-file-rendered .markdown-content'))
      .toContainText('You made it from the repo.');
  });
});
