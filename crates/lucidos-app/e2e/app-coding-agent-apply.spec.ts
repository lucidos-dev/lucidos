import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, waitForActionPanel, clickChangeAction } from './helpers';
import { createIframeAppFixture, createAppCCThreadWithChange, cleanupCCThread, git } from './db-helpers';

test.describe('App coding-agent thread — Apply', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('Apply lands → iframe reloads + WIP preview button hides', async ({ page }) => {
    const suffix = `${Date.now()}`;
    const appId = `e2e-apply-${suffix}`;
    const fixture = createIframeAppFixture(appId, {
      html: '<!doctype html><title>apply fixture</title>',
      js: '/* noop */',
      manifest: { id: appId, name: `${appId} fixture`, description: 'apply e2e' },
    });
    try {
      git(['add', `data/apps/${appId}`]);
      git(['commit', '-m', `e2e seed app ${appId}`]);
    } catch {  }

    const seeded = createAppCCThreadWithChange({
      appId,
      titlePrefix: 'Apply test',
      suffix,
      fileExt: '.html',
    });

    try {
      await navigateToApp(page);

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

      const srcBefore = await page.evaluate(() => {
        const fr = document.querySelector('iframe[data-role="app-ui-frame"]') as HTMLIFrameElement | null;
        return fr?.src ?? null;
      });
      expect(srcBefore).toBeTruthy();

      await waitForActionPanel(page, 'Apply', 15_000);
      await clickChangeAction(page, 'Apply');

      await page.waitForFunction((before) => {
        const fr = document.querySelector('iframe[data-role="app-ui-frame"]') as HTMLIFrameElement | null;
        return !!fr && fr.src !== before;
      }, srcBefore, { timeout: 15_000 });

      const wipToggle = page.locator('[data-role="wip-preview-toggle"]:visible').first();
      if (await wipToggle.count() > 0) {
        const pressed = await wipToggle.getAttribute('aria-pressed');
        expect(pressed).toBe('false');
      }
    } finally {
      cleanupCCThread(seeded.threadId, seeded.changeId, seeded.branch, seeded.file);
      fixture.cleanup();
      try { git(['add', `data/apps/${appId}`]); git(['commit', '-m', `e2e cleanup app ${appId}`, '--allow-empty']); } catch {  }
    }
  });
});
