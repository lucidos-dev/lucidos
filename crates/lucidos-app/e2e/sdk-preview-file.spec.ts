import { test, expect } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE, git } from './db-helpers';
import { apiRequest, clickHeaderAction, ensureMobileView, gotoWithRetry } from './helpers';

const APP_ID = 'e2e-sdk-preview-test';
const APP_DIR = resolve(WORKSPACE, 'data/apps', APP_ID);
const SAMPLE_NAME = 'e2e-preview-sample.txt';
const SAMPLE_PATH = `artifacts/${SAMPLE_NAME}`;
const SAMPLE_FILE = resolve(WORKSPACE, 'data', SAMPLE_PATH);
const CITED_LINE = 4;
const CITED_TEXT = 'the line the report cites';
// A rendered markdown artifact whose sibling link routes the shell, and the file
// it links to. Used for the "a link inside the glance navigates" case.
const DOC_NAME = 'e2e-preview-doc.md';
const OTHER_NAME = 'e2e-preview-other.md';
const DOC_FILE = resolve(WORKSPACE, 'data/artifacts', DOC_NAME);
const OTHER_FILE = resolve(WORKSPACE, 'data/artifacts', OTHER_NAME);

/** `lucidos.ui.previewFile` shows a cited file OVER the app: the reader glances
 *  at it and carries on, instead of being navigated into the Files panel and
 *  having to find their way back. So every case here checks two things at once:
 *  the file is on screen, and the app is still behind it. */
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

  // Same restore-on-load path as sdk-confirm.spec.ts: seeding `app-window-open`
  // before navigation makes loadApps() open this app, which mounts the host's
  // Preact tree AND the app iframe the host's message listener whitelists.
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

    // Rendered by the HOST, over the app, not inside the iframe.
    await expect(modal(page)).toBeVisible();
    await expect(page.locator('.file-preview-modal-name'))
      .toHaveText(`${SAMPLE_NAME}:${CITED_LINE}`);
    await expect(page.locator('.file-preview-modal-detail')).toHaveText(SAMPLE_PATH);

    // The file itself, with the cited line selected the way a line click selects
    // it (same LineNumberedCode the Files panel renders).
    const cited = modal(page).locator(`.code-line[data-line="${CITED_LINE}"]`);
    await expect(cited).toContainText(CITED_TEXT);
    await expect(cited).toHaveClass(/line-selected/);

    // The whole point: the app is still there behind the glance.
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

  // A citation's line number is the part that goes stale. It must never cost the
  // reader the file itself.
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

    // The preview was requested from inside the app iframe, so on WebKit the
    // iframe still holds the page's keyboard focus and a programmatic focus on a
    // host element does not move it (see the same note in sdk-confirm.spec.ts).
    // A real pointer interaction does: click the non-interactive path label
    // first (an inside-panel click, so it neither dismisses nor activates
    // anything), then press Escape.
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

    // Viewport corner: outside the centered panel (60rem wide at most).
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

  // The escalation: a glance the reader decides to promote into the real thing.
  test('Open in Files lands on the full preview of the same file', async ({ page }) => {
    const { frame } = await setupIframe(page);
    await preview(frame, { file_path: SAMPLE_PATH, line: CITED_LINE });
    await expect(modal(page)).toBeVisible();

    await modal(page).locator('button', { hasText: 'Open in Files' }).click();

    await expect(modal(page)).toHaveCount(0);
    const inlinePreview = page.locator('.file-preview-inline:visible');
    await expect(inlinePreview).toBeVisible();
    await expect(inlinePreview.locator(`.code-line[data-line="${CITED_LINE}"]`))
      .toHaveClass(/line-selected/);
    // The app it was glanced from is behind us now: this is a real navigation.
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toHaveCount(0);
  });

  // A rendered markdown artifact's own links route the shell (handlePreviewLinkClick).
  // The glance must not hang over a pane that has moved on, and the file it
  // landed on must keep the state its opener set, not the panel's pre-modal one.
  test('a link inside the glance navigates the shell and closes the glance', async ({ page }) => {
    const { frame } = await setupIframe(page);
    await preview(frame, { file_path: `artifacts/${DOC_NAME}` });
    await expect(modal(page)).toBeVisible();

    await modal(page).locator('a', { hasText: 'the other file' }).click();

    await expect(modal(page)).toHaveCount(0);
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

  // The reported bug: in fullscreen the click did nothing and the promise
  // resolved anyway. Two different fullscreens fail for two different reasons,
  // so both are driven here through the one affordance a reader uses.
  test.describe('with the app in fullscreen', () => {
    /** Click the header's fullscreen toggle and wait for the app to be in one
     *  of the two fullscreen states. Chromium grants the real Fullscreen API;
     *  WebKit refuses it for a non-video element and the host falls back to
     *  pseudo-fullscreen, so the same click exercises the other half there. */
    async function enterFullscreen(page: import('@playwright/test').Page) {
      // The toggle lives in the CONTENT header, which on a phone is only
      // rendered for the content view; the app pane being on screen is not the
      // same thing as the header showing that pane's actions.
      await ensureMobileView(page, 'content');
      // An app-UI overlay contributes three context actions (refresh, open in
      // a tab, fullscreen), and three fold whole into the `⋯` menu at any
      // width, so the toggle has no header button on ANY project here.
      // `clickHeaderAction` finds it in either placement.
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

    /** What the host had to work with, for a failure message worth reading: a
     *  refusal here means the fullscreen element and the overlay mount
     *  disagreed, and which of them is wrong is the whole diagnosis. */
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

      // Native fullscreen paints ONLY the fullscreen element's subtree, so a
      // modal that is merely on top is still invisible: it has to be INSIDE.
      // Under the pseudo fallback the panel is painted in the normal layer and
      // the modal stays at the app root, above it by z-index.
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

      // A pointer dismissal must not cost the reader their fullscreen, in
      // either mechanism. (It also proves the portaled scrim and panel are
      // still interactive inside `.app-shell`, where the inert-behind rule
      // would otherwise have reached them.)
      await modal(page).locator('button[aria-label="Close preview"]').click();

      await expect(modal(page)).toHaveCount(0);
      const stillFullscreen = await panel.evaluate((el) =>
        document.fullscreenElement === el || el.classList.contains('app-ui-fullscreen'));
      expect(stillFullscreen).toBe(true);
    });

    // One Escape, one effect, in both mechanisms but not the same effect.
    //
    // Under the pseudo fallback the whole thing is ours: the LIFO overlay stack
    // pops the modal and leaves fullscreen alone.
    //
    // Under native fullscreen the browser claims the key to exit and no handler
    // can stop it, so the host stands down and the modal survives into the
    // normal layout for the next Escape. Only the host half is asserted here:
    // the UA's fullscreen exit is driven by real key input, and a CDP-injected
    // Escape does not trigger it in headless Chromium, so asserting the exit
    // would be testing the harness. The modal surviving IS the half we own, and
    // it is the one that regressed while this test was being written (the
    // overlay's own bubble-phase Escape closed it behind the stand-down's back).
    test('one Escape does not both close the preview and drop fullscreen', async ({ page }) => {
      const { frame } = await setupIframe(page);
      const panel = await enterFullscreen(page);
      const wasNative = await panel.evaluate((el) => document.fullscreenElement === el);
      await preview(frame, { file_path: SAMPLE_PATH });
      await expect(modal(page)).toBeVisible();

      await page.locator('.file-preview-modal-detail').click();
      await page.keyboard.press('Escape');

      if (wasNative) {
        // Deliberately not a `toBeVisible` race: give the dismissal a chance to
        // land wrongly before asserting it did not.
        await page.waitForTimeout(500);
        await expect(modal(page)).toBeVisible();
      } else {
        await expect(modal(page)).toHaveCount(0);
        await expect(panel).toHaveClass(/app-ui-fullscreen/);
      }
    });

    // Same layer, same bug: a toast is host-rendered too, and an app calling
    // toast() while fullscreen saw nothing.
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

  /** The revision problem this locator form exists for: a file whose edits live
   *  only on a coding agent's worktree branch previews without them at `HEAD`.
   *  The Files panel can read a branch, but only for the repository it is bound
   *  to, and the modal may be showing another one. So the caller names the
   *  revision, and the content it gets back must be the branch's. */
  test.describe('at a revision the locator names', () => {
    const suffix = Date.now().toString(36);
    const REPO_FILE = `e2e-preview-ref-${suffix}.txt`;
    const BRANCH = `e2e-test/preview-ref-${suffix}`;
    const ON_BRANCH = 'the content only the branch has';
    let repoId: string;

    // The file exists ONLY on the branch, which is the shape this feature is
    // for: a coding agent's worktree edits are not at the clone's HEAD. It also
    // makes the two cases below unambiguous without writing anything to main.
    test.beforeAll(() => {
      git(['checkout', '-b', BRANCH, 'main']);
      writeFileSync(resolve(WORKSPACE, REPO_FILE), `${ON_BRANCH}\n`);
      git(['add', REPO_FILE]);
      git(['commit', '-m', `e2e preview-ref fixture ${suffix}`]);
      git(['checkout', 'main']);
    });

    test.afterAll(() => {
      try { git(['branch', '-D', BRANCH]); } catch { /* already gone */ }
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

    // The bug this form fixes, shown from the other side: without a ref the
    // locator means the clone's HEAD, where this file does not exist at all.
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
      // Named by its repo-relative path, not the encoding: the ref is part of
      // the locator, not part of the file's name.
      await expect(page.locator('.file-preview-modal-detail')).toHaveText(REPO_FILE);
    });

    test('honours a cited line at that revision', async ({ page }) => {
      const { frame } = await setupIframe(page);
      await preview(frame, { file_path: `repo:${repoId}:file#${BRANCH}:${REPO_FILE}`, line: 1 });

      const cited = modal(page).locator('.code-line[data-line="1"]');
      await expect(cited).toContainText(ON_BRANCH);
      await expect(cited).toHaveClass(/line-selected/);
    });

    // An empty ref is a malformed locator, not "unqualified": it degrades to the
    // artifact rule rather than silently reading HEAD.
    test('degrades a locator with an empty ref to a data path', async ({ page }) => {
      const { frame } = await setupIframe(page);
      await preview(frame, { file_path: `repo:${repoId}:file#:${REPO_FILE}` });

      await expect(modal(page)).toBeVisible();
      await expect(page.locator('.file-preview-modal-detail'))
        .toHaveText(`artifacts/repo:${repoId}:file#:${REPO_FILE}`);
    });
  });
});
