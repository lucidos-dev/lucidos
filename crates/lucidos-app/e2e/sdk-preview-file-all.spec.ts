import { test, expect } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE, git } from './db-helpers';
import {
  apiRequest, clickHeaderAction, ensureMobileView, gotoWithRetry, waitForModalExitFade,
} from './helpers';

const APP_ID = 'e2e-sdk-preview-test';
const APP_DIR = resolve(WORKSPACE, 'data/apps', APP_ID);
const SAMPLE_NAME = 'e2e-preview-sample.txt';
const SAMPLE_PATH = `artifacts/${SAMPLE_NAME}`;
const SAMPLE_FILE = resolve(WORKSPACE, 'data', SAMPLE_PATH);
const CITED_LINE = 4;
const CITED_TEXT = 'the line the report cites';
const DOC_NAME = 'e2e-preview-doc.md';
const OTHER_NAME = 'e2e-preview-other.md';
const DOC_FILE = resolve(WORKSPACE, 'data/artifacts', DOC_NAME);
const OTHER_FILE = resolve(WORKSPACE, 'data/artifacts', OTHER_NAME);

test.describe('SDK lucidos.ui.previewFile: a file preview over the app', () => {
  test.beforeAll(() => {
    mkdirSync(APP_DIR, { recursive: true });
    writeFileSync(resolve(APP_DIR, 'index.html'), `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>SDK previewFile test</title>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<button id="cite">Cite</button>
<div id="result">none</div>
<script>
  // Expose a helper so the test can preview any locator + line.
  window.runPreview = async function(params) {
    try {
      await lucidos.ui.previewFile(params);
      document.getElementById('result').textContent = 'shown';
      return 'shown';
    } catch (e) {
      document.getElementById('result').textContent = 'refused: ' + e.message;
      return 'refused';
    }
  };
</script>
</body>
</html>
`);
    writeFileSync(resolve(APP_DIR, 'manifest.json'), JSON.stringify({
      id: APP_ID,
      name: 'SDK previewFile test',
      description: 'e2e fixture',
    }));

    mkdirSync(resolve(WORKSPACE, 'data/artifacts'), { recursive: true });
    const lines = Array.from({ length: 12 }, (_, i) =>
      i + 1 === CITED_LINE ? CITED_TEXT : `filler line ${i + 1}`);
    writeFileSync(SAMPLE_FILE, `${lines.join('\n')}\n`);

    writeFileSync(DOC_FILE, `# Preview doc\n\nSee [the other file](${OTHER_NAME}) for more.\n`);
    writeFileSync(OTHER_FILE, '# The other file\n\nLanded here.\n');
  });

  test.afterAll(() => {
    rmSync(APP_DIR, { recursive: true, force: true });
    rmSync(SAMPLE_FILE, { force: true });
    rmSync(DOC_FILE, { force: true });
    rmSync(OTHER_FILE, { force: true });
  });

  async function setupIframe(page: import('@playwright/test').Page) {
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await gotoWithRetry(page, '/');
    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10000 });
    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#cite')).toBeVisible({ timeout: 10000 });
    const handle = await iframeLoc.elementHandle();
    if (!handle) throw new Error('iframe handle missing');
    const frame = await handle.contentFrame();
    if (!frame) throw new Error('iframe contentFrame missing');
    return { loc: appFrame, frame, iframeLoc };
  }

  function preview(frame: import('@playwright/test').Frame, params: unknown) {
    return frame.evaluate(
      (p) => (window as unknown as { runPreview: (o: unknown) => Promise<string> }).runPreview(p),
      params,
    );
  }

  const modal = (page: import('@playwright/test').Page) =>
    page.locator('[data-role="file-preview-modal"]');

  test('shows the cited file at its line, without navigating away from the app', async ({ page }) => {
    const { frame, iframeLoc } = await setupIframe(page);

    const shown = preview(frame, { file_path: SAMPLE_PATH, line: CITED_LINE });

    await expect(modal(page)).toBeVisible();
    await expect(page.locator('.file-preview-modal-name'))
      .toHaveText(`${SAMPLE_NAME}:${CITED_LINE}`);
    await expect(page.locator('.file-preview-modal-detail')).toHaveText(SAMPLE_PATH);

    const cited = modal(page).locator(`.code-line[data-line="${CITED_LINE}"]`);
    await expect(cited).toContainText(CITED_TEXT);
    await expect(cited).toHaveClass(/line-selected/);

    await expect(iframeLoc).toBeVisible();
    expect(await shown).toBe('shown');
  });

  test('opens at the top when no line is cited, selecting nothing', async ({ page }) => {
    const { frame } = await setupIframe(page);
    await preview(frame, { file_path: SAMPLE_PATH });

    await expect(modal(page)).toBeVisible();
    await expect(page.locator('.file-preview-modal-name')).toHaveText(SAMPLE_NAME);
    await expect(modal(page).locator('.line-selected')).toHaveCount(0);
  });

  test('still shows the file when the cited line is stale', async ({ page }) => {
    const { frame } = await setupIframe(page);
    const shown = await preview(frame, { file_path: SAMPLE_PATH, line: 0 });

    expect(shown).toBe('shown');
    await expect(modal(page)).toBeVisible();
    await expect(modal(page).locator('.line-selected')).toHaveCount(0);
  });

  test('the close control dismisses it and leaves the app in place', async ({ page }) => {
    const { frame, iframeLoc } = await setupIframe(page);
    await preview(frame, { file_path: SAMPLE_PATH, line: CITED_LINE });
    await expect(modal(page)).toBeVisible();

    await modal(page).locator('button[aria-label="Close preview"]').click();
    await expect(modal(page)).toHaveCount(0);
    await expect(iframeLoc).toBeVisible();
  });

  test('Escape dismisses it', async ({ page }) => {
    const { frame } = await setupIframe(page);
    await preview(frame, { file_path: SAMPLE_PATH });
    await expect(modal(page)).toBeVisible();

    await page.locator('.file-preview-modal-detail').click();
    await page.keyboard.press('Escape');
    await expect(modal(page)).toHaveCount(0);
  });

  test('a backdrop click dismisses it', async ({ page }, testInfo) => {
    test.skip(
      testInfo.project.name !== 'chromium',
      'the modal is full-bleed on a phone, so there is no backdrop to click',
    );
    const { frame } = await setupIframe(page);
    await preview(frame, { file_path: SAMPLE_PATH });
    await expect(modal(page)).toBeVisible();

    await page.mouse.click(2, 2);
    await expect(modal(page)).toHaveCount(0);
  });

  test('a second preview replaces the first', async ({ page }) => {
    const { frame } = await setupIframe(page);
    await preview(frame, { file_path: SAMPLE_PATH });
    await expect(page.locator('.file-preview-modal-name')).toHaveText(SAMPLE_NAME);

    await preview(frame, { file_path: SAMPLE_PATH, line: CITED_LINE });
    await expect(page.locator('.file-preview-modal-name')).toHaveText(`${SAMPLE_NAME}:${CITED_LINE}`);
    await expect(modal(page)).toHaveCount(1);
  });

  test('Open in Files lands on the full preview of the same file', async ({ page }) => {
    const { frame } = await setupIframe(page);
    await preview(frame, { file_path: SAMPLE_PATH, line: CITED_LINE });
    await expect(modal(page)).toBeVisible();

    await modal(page).locator('button', { hasText: 'Open in Files' }).click();

    await expect(modal(page)).toHaveCount(0);
    await waitForModalExitFade(page);
    const inlinePreview = page.locator('.file-preview-inline:visible');
    await expect(inlinePreview).toBeVisible();
    await expect(inlinePreview.locator(`.code-line[data-line="${CITED_LINE}"]`))
      .toHaveClass(/line-selected/);
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toHaveCount(0);
  });

  test('a link inside the glance navigates the shell and closes the glance', async ({ page }) => {
    const { frame } = await setupIframe(page);
    await preview(frame, { file_path: `artifacts/${DOC_NAME}` });
    await expect(modal(page)).toBeVisible();

    await modal(page).locator('a', { hasText: 'the other file' }).click();

    await expect(modal(page)).toHaveCount(0);
    await waitForModalExitFade(page);
    const inlinePreview = page.locator('.file-preview-inline:visible');
    await expect(inlinePreview).toContainText('The other file');
    await expect(inlinePreview.locator('.line-selected')).toHaveCount(0);
  });

  test('refuses a request with no file_path, and shows nothing', async ({ page }) => {
    const { frame } = await setupIframe(page);
    const result = await preview(frame, { file_path: '' });

    expect(result).toBe('refused');
    await expect(modal(page)).toHaveCount(0);
  });

  test.describe('with the app in fullscreen', () => {
    async function enterFullscreen(page: import('@playwright/test').Page) {
      await ensureMobileView(page, 'content');
      await clickHeaderAction(page, '.app-fullscreen');
      const panel = page.locator('[data-role="app-ui-panel"]:visible');
      await expect(async () => {
        const state = await panel.evaluate((el) => ({
          native: document.fullscreenElement === el,
          pseudo: el.classList.contains('app-ui-fullscreen'),
        }));
        expect(state.native || state.pseudo).toBe(true);
      }).toPass({ timeout: 5000 });
      return panel;
    }

    const fullscreenState = (page: import('@playwright/test').Page) => page.evaluate(() => {
      const fs = document.fullscreenElement;
      const mounts = document.querySelectorAll('[data-overlay-layer]');
      return {
        fullscreen: fs ? `${fs.tagName}.${fs.className}[${fs.getAttribute('data-role') ?? ''}]` : null,
        fullscreenConnected: fs?.isConnected ?? null,
        panels: document.querySelectorAll('[data-role="app-ui-panel"]').length,
        mounts: mounts.length,
        containsMount: !!fs && mounts.length > 0 && fs.contains(mounts[0]),
      };
    });

    test('shows the preview over the app, and the app is still fullscreen', async ({ page }) => {
      const { frame } = await setupIframe(page);
      const panel = await enterFullscreen(page);

      const shown = await preview(frame, { file_path: SAMPLE_PATH, line: CITED_LINE });

      expect(shown, `host state: ${JSON.stringify(await fullscreenState(page))}`).toBe('shown');
      await expect(modal(page)).toBeVisible();
      await expect(modal(page).locator(`.code-line[data-line="${CITED_LINE}"]`))
        .toContainText(CITED_TEXT);

      const placement = await modal(page).evaluate((el) => {
        const p = document.querySelector('[data-role="app-ui-panel"]');
        return { insidePanel: !!p?.contains(el), nativelyFullscreen: document.fullscreenElement === p };
      });
      expect(placement.insidePanel || !placement.nativelyFullscreen).toBe(true);
      await expect(panel).toBeVisible();
    });

    test('the close control dismisses it without leaving fullscreen', async ({ page }) => {
      const { frame } = await setupIframe(page);
      const panel = await enterFullscreen(page);
      await preview(frame, { file_path: SAMPLE_PATH });
      await expect(modal(page)).toBeVisible();

      await modal(page).locator('button[aria-label="Close preview"]').click();

      await expect(modal(page)).toHaveCount(0);
      const stillFullscreen = await panel.evaluate((el) =>
        document.fullscreenElement === el || el.classList.contains('app-ui-fullscreen'));
      expect(stillFullscreen).toBe(true);
    });

    test('one Escape does not both close the preview and drop fullscreen', async ({ page }) => {
      const { frame } = await setupIframe(page);
      const panel = await enterFullscreen(page);
      const wasNative = await panel.evaluate((el) => document.fullscreenElement === el);
      await preview(frame, { file_path: SAMPLE_PATH });
      await expect(modal(page)).toBeVisible();

      await page.locator('.file-preview-modal-detail').click();
      await page.keyboard.press('Escape');

      if (wasNative) {
        await page.waitForTimeout(500);
        await expect(modal(page)).toBeVisible();
      } else {
        await expect(modal(page)).toHaveCount(0);
        await expect(panel).toHaveClass(/app-ui-fullscreen/);
      }
    });

    test('a toast from the app is visible too', async ({ page }) => {
      const { frame } = await setupIframe(page);
      await enterFullscreen(page);

      await frame.evaluate(() => {
        (window as unknown as { lucidos: { ui: { toast: (m: string) => void } } })
          .lucidos.ui.toast('fullscreen toast');
      });

      await expect(page.locator('.toast-container .toast-body')).toContainText('fullscreen toast');
    });
  });

  test.describe('at a revision the locator names', () => {
    const suffix = Date.now().toString(36);
    const REPO_FILE = `e2e-preview-ref-${suffix}.txt`;
    const BRANCH = `e2e-test/preview-ref-${suffix}`;
    const ON_BRANCH = 'the content only the branch has';
    let repoId: string;

    test.beforeAll(() => {
      git(['checkout', '-b', BRANCH, 'main']);
      writeFileSync(resolve(WORKSPACE, REPO_FILE), `${ON_BRANCH}\n`);
      git(['add', REPO_FILE]);
      git(['commit', '-m', `e2e preview-ref fixture ${suffix}`]);
      git(['checkout', 'main']);
    });

    test.afterAll(() => {
      try { git(['branch', '-D', BRANCH]); } catch {  }
    });

    test.beforeEach(async ({ page }) => {
      const resp = await apiRequest(page).post('/api/v1/repositories', {
        data: { name: `e2e-preview-ref-${suffix}`, path: WORKSPACE, description: 'e2e test repo' },
      });
      expect(resp.ok()).toBeTruthy();
      repoId = (await resp.json()).id;
    });

    test.afterEach(async ({ page }) => {
      if (repoId) await apiRequest(page).delete(`/api/v1/repositories/${repoId}`);
    });

    test('reads the clone HEAD when the locator names no revision', async ({ page }) => {
      const { frame } = await setupIframe(page);
      await preview(frame, { file_path: `repo:${repoId}:file:${REPO_FILE}` });

      await expect(modal(page)).toBeVisible();
      await expect(modal(page).locator('.error-text')).toBeVisible();
    });

    test('reads the branch when the locator names it', async ({ page }) => {
      const { frame } = await setupIframe(page);
      const shown = await preview(frame, { file_path: `repo:${repoId}:file#${BRANCH}:${REPO_FILE}` });

      expect(shown).toBe('shown');
      await expect(modal(page)).toBeVisible();
      await expect(modal(page).locator('.repo-file-content')).toContainText(ON_BRANCH);
      await expect(page.locator('.file-preview-modal-detail')).toHaveText(REPO_FILE);
    });

    test('honours a cited line at that revision', async ({ page }) => {
      const { frame } = await setupIframe(page);
      await preview(frame, { file_path: `repo:${repoId}:file#${BRANCH}:${REPO_FILE}`, line: 1 });

      const cited = modal(page).locator('.code-line[data-line="1"]');
      await expect(cited).toContainText(ON_BRANCH);
      await expect(cited).toHaveClass(/line-selected/);
    });

    test('degrades a locator with an empty ref to a data path', async ({ page }) => {
      const { frame } = await setupIframe(page);
      await preview(frame, { file_path: `repo:${repoId}:file#:${REPO_FILE}` });

      await expect(modal(page)).toBeVisible();
      await expect(page.locator('.file-preview-modal-detail'))
        .toHaveText(`artifacts/repo:${repoId}:file#:${REPO_FILE}`);
    });
  });
});
