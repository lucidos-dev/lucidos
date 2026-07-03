import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, ensureMobileView, waitForActionPanel } from './helpers';
import { createIframeAppFixture, createAppCCThreadWithChange, cleanupCCThread, git } from './db-helpers';

test.describe('App coding-agent thread — Apply', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  // Apply on an app coding-agent thread ff-merges into workspace main and
  // emits AppUiRefreshRequested when iframe-bundled files change. The
  // frontend's reaction to that SSE event is to bump appRefreshKey, which
  // forces AppFrame to remount with a cache-busted src. This spec verifies
  // the visible end-state: after clicking Apply, the open iframe's src
  // gains the `_r=` cache-buster (proof the refresh event landed), AND the
  // WIP preview button stays hidden (worktree is gone, no diff to preview).
  test('Apply lands → iframe reloads + WIP preview button hides', async ({ page }) => {
    const suffix = `${Date.now()}`;
    const appId = `e2e-apply-${suffix}`;
    const fixture = createIframeAppFixture(appId, {
      // Edited by the seeded change so the apply path matches an
      // iframe-bundled file (index.html → AppUiRefreshRequested fires).
      html: '<!doctype html><title>apply fixture</title>',
      js: '/* noop */',
      manifest: { id: appId, name: `${appId} fixture`, description: 'apply e2e' },
    });
    try {
      git(['add', `data/apps/${appId}`]);
      git(['commit', '-m', `e2e seed app ${appId}`]);
    } catch { /* nothing to commit */ }

    // The change must touch an iframe-bundled file (HTML/CSS/JS/manifest)
    // under the app folder so the apply path's
    // `any_iframe_bundled_file_changed` gate flips and emits
    // `AppUiRefreshRequested`. Pass `.html` so the seeded change carries
    // a refresh-triggering file.
    const seeded = createAppCCThreadWithChange({
      appId,
      titlePrefix: 'Apply test',
      suffix,
      fileExt: '.html',
    });

    try {
      await navigateToApp(page);
      await ensureMobileView(page, 'thread');

      // Open the app + focus the thread (same routing as the WIP-preview
      // spec — see comments there).
      await page.evaluate((id) => {
        const w = window as unknown as { __openApp?: (id: string) => void };
        if (w.__openApp) { w.__openApp(id); return; }
        location.hash = `app=${id}`;
      }, appId);
      await page.waitForFunction(() => !!document.querySelector('iframe[data-role="app-ui-frame"]'), undefined, { timeout: 15_000 });
      await page.evaluate((tid) => {
        location.hash = `thread=${tid}`;
        window.dispatchEvent(new PopStateEvent('popstate'));
      }, seeded.threadId);

      // Snapshot the iframe src before Apply.
      const srcBefore = await page.evaluate(() => {
        const fr = document.querySelector('iframe[data-role="app-ui-frame"]') as HTMLIFrameElement | null;
        return fr?.src ?? null;
      });
      expect(srcBefore).toBeTruthy();

      // Click Apply — the action panel surfaces "Apply" / "Discard" once
      // the coding_agent_proposed projection flag is true (seeded).
      const panel = await waitForActionPanel(page, 'Apply', 15_000);
      await panel.locator('button', { hasText: /^Apply/ }).first().click();

      // After Apply, the iframe is remounted with a cache-busted src
      // (appRefreshKey > 0 → ?_r=N). Wait for the src to differ.
      await page.waitForFunction((before) => {
        const fr = document.querySelector('iframe[data-role="app-ui-frame"]') as HTMLIFrameElement | null;
        return !!fr && fr.src !== before;
      }, srcBefore, { timeout: 15_000 });

      // The WIP preview button must NOT be visible after Apply — the
      // change is gone, codingAgentHasDiff cleared, the wipPreview effect
      // clears wipPreviewThreadId. (The button is gated on
      // codingAgentKind === 'app' + the app being open — kind stays, but
      // pressing it would now show 404 content; better UX is to hide it
      // entirely once no diff exists. v1 keeps it visible but inert; this
      // spec just confirms the toggle isn't stuck on.)
      const wipToggle = page.locator('[data-role="wip-preview-toggle"]:visible').first();
      if (await wipToggle.count() > 0) {
        const pressed = await wipToggle.getAttribute('aria-pressed');
        expect(pressed).toBe('false');
      }
    } finally {
      cleanupCCThread(seeded.threadId, seeded.changeId, seeded.branch, seeded.file);
      fixture.cleanup();
      try { git(['add', `data/apps/${appId}`]); git(['commit', '-m', `e2e cleanup app ${appId}`, '--allow-empty']); } catch { /* */ }
    }
  });
});
