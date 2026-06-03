import { test, expect } from '@playwright/test';
import { navigateToApp, assertHealthy, ensureMobileView } from './helpers';
import { createIframeAppFixture, createAppCCThreadWithChange, cleanupCCThread, git } from './db-helpers';

test.describe('App coding-agent thread — WIP app preview toggle', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('toggle swaps panel-overlay iframe between live and ?thread_id=<id>', async ({ page }) => {
    const suffix = `${Date.now()}`;
    const appId = `e2e-wip-${suffix}`;
    const fixture = createIframeAppFixture(appId, {
      html: '<!doctype html><title>wip preview fixture</title>',
      js: '/* noop */',
      manifest: { id: appId, name: `${appId} fixture`, description: 'wip preview e2e' },
    });
    // The change creator commits to main, so it needs the app folder already
    // tracked. One commit, idempotent if the file's already on main.
    try {
      git(['add', `data/apps/${appId}`]);
      git(['commit', '-m', `e2e seed app ${appId}`]);
    } catch { /* nothing to commit — idempotent */ }

    const seeded = createAppCCThreadWithChange({
      appId,
      titlePrefix: 'WIP preview test',
      suffix,
    });

    try {
      await navigateToApp(page);
      await ensureMobileView(page, 'thread');

      // Open the app in the panel-overlay via the engine's app URL.
      // navigateToApp leaves us on the landing page; openApp(app) is the
      // production path but requires the apps list to be loaded — easier to
      // just navigate the URL the SDK builds.
      await page.evaluate((id) => {
        const w = window as unknown as { __openApp?: (id: string) => void };
        if (w.__openApp) { w.__openApp(id); return; }
        // Fallback: dispatch a hashchange so the route picks it up.
        location.hash = `app=${id}`;
      }, appId);
      // Wait for the iframe to mount with the live (no thread_id) URL.
      const liveSrcRegex = new RegExp(`/app/${appId}/(?!.*thread_id=)`);
      await page.waitForFunction((r) => {
        const re = new RegExp(r);
        const fr = document.querySelector('iframe[data-role="app-ui-frame"]') as HTMLIFrameElement | null;
        return !!fr && re.test(fr.src);
      }, liveSrcRegex.source, { timeout: 15_000 });

      // Focus the seeded coding-agent thread.
      await page.evaluate((tid) => {
        location.hash = `thread=${tid}`;
        // Belt + suspenders: also dispatch popstate so the router fires.
        window.dispatchEvent(new PopStateEvent('popstate'));
      }, seeded.threadId);

      // The WIP preview toggle button appears when (a) focused thread is an
      // app cc thread AND (b) the panel-overlay shows that app.
      const toggle = page.locator('[data-role="wip-preview-toggle"]:visible').first();
      await expect(toggle).toBeVisible({ timeout: 10_000 });

      // Click → iframe contentWindow navigates to `?thread_id=<id>`.
      // `navigateAppIframe` uses `contentWindow.location.replace` to avoid
      // session-history pollution (WebKit #9166), which does NOT update the
      // iframe's `src` attribute — so we read `contentWindow.location.href`.
      // The fixture and the WIP URL are same-origin (engine serves both),
      // so cross-origin access doesn't throw.
      await toggle.click();
      await page.waitForFunction((tid) => {
        const fr = document.querySelector('iframe[data-role="app-ui-frame"]') as HTMLIFrameElement | null;
        const href = fr?.contentWindow?.location?.href ?? fr?.src ?? '';
        return href.includes(`thread_id=${tid}`);
      }, seeded.threadId, { timeout: 5_000 });

      // Click again → reverts to live.
      await toggle.click();
      await page.waitForFunction(() => {
        const fr = document.querySelector('iframe[data-role="app-ui-frame"]') as HTMLIFrameElement | null;
        const href = fr?.contentWindow?.location?.href ?? fr?.src ?? '';
        return href.length > 0 && !href.includes('thread_id=');
      }, undefined, { timeout: 5_000 });
    } finally {
      cleanupCCThread(seeded.threadId, seeded.changeId, seeded.branch, seeded.file);
      fixture.cleanup();
      try { git(['add', `data/apps/${appId}`]); git(['commit', '-m', `e2e cleanup app ${appId}`, '--allow-empty']); } catch { /* */ }
    }
  });
});
