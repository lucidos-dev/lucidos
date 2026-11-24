import { test, expect, type Page } from './fixtures';
import {
  navigateToApp, assertHealthy, composerToggleState, ensureMobileView, pressComposerControl,
} from './helpers';
import { createIframeAppFixture, createAppCCThreadWithChange, cleanupCCThread, git } from './db-helpers';

function appFrameUrl(page: import('@playwright/test').Page): string {
  return page.frames().map((f) => f.url()).find((u) => u.includes('/app/')) ?? '';
}

const WIP_TOGGLE = '[data-role="wip-preview-toggle"]';

async function expectWipToggle(page: Page, pressed?: boolean): Promise<void> {
  await expect(async () => {
    const state = await composerToggleState(page, WIP_TOGGLE);
    expect(state).not.toBeNull();
    if (pressed !== undefined) expect(state).toBe(String(pressed));
  }).toPass({ timeout: 10_000 });
}

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
    try {
      git(['add', `data/apps/${appId}`]);
      git(['commit', '-m', `e2e seed app ${appId}`]);
    } catch {  }

    const seeded = createAppCCThreadWithChange({
      appId,
      titlePrefix: 'WIP preview test',
      suffix,
    });

    try {
      await navigateToApp(page);
      await ensureMobileView(page, 'thread');

      await page.evaluate((id) => {
        const w = window as unknown as { __openApp?: (id: string) => void };
        if (w.__openApp) { w.__openApp(id); return; }
        location.hash = `app=${id}`;
      }, appId);
      const liveSrcRegex = new RegExp(`/app/${appId}/(?!.*thread_id=)`);
      await page.waitForFunction((r) => {
        const re = new RegExp(r);
        const fr = document.querySelector('iframe[data-role="app-ui-frame"]') as HTMLIFrameElement | null;
        return !!fr && re.test(fr.src);
      }, liveSrcRegex.source, { timeout: 15_000 });

      await page.evaluate((tid) => {
        location.hash = `thread=${tid}`;
        window.dispatchEvent(new PopStateEvent('popstate'));
      }, seeded.threadId);

      await expectWipToggle(page);

      await pressComposerControl(page, WIP_TOGGLE);
      await expect.poll(() => appFrameUrl(page), { timeout: 5_000 })
        .toContain(`thread_id=${seeded.threadId}`);

      await pressComposerControl(page, WIP_TOGGLE);
      await expect.poll(() => appFrameUrl(page), { timeout: 5_000 })
        .not.toContain('thread_id=');
    } finally {
      cleanupCCThread(seeded.threadId, seeded.changeId, seeded.branch, seeded.file);
      fixture.cleanup();
      try { git(['add', `data/apps/${appId}`]); git(['commit', '-m', `e2e cleanup app ${appId}`, '--allow-empty']); } catch {  }
    }
  });

  test('toggle is reachable without opening the app first; click opens app + WIP', async ({ page }) => {
    const suffix = `${Date.now()}`;
    const appId = `e2e-wip-noopen-${suffix}`;
    const fixture = createIframeAppFixture(appId, {
      html: '<!doctype html><title>wip preview noopen fixture</title>',
      js: '/* noop */',
      manifest: { id: appId, name: `${appId} fixture`, description: 'wip preview noopen e2e' },
    });
    try {
      git(['add', `data/apps/${appId}`]);
      git(['commit', '-m', `e2e seed app ${appId}`]);
    } catch {  }

    const seeded = createAppCCThreadWithChange({
      appId,
      titlePrefix: 'WIP preview noopen test',
      suffix,
    });

    try {
      await navigateToApp(page);
      await ensureMobileView(page, 'thread');

      await page.evaluate((tid) => {
        location.hash = `thread=${tid}`;
        window.dispatchEvent(new PopStateEvent('popstate'));
      }, seeded.threadId);

      await expectWipToggle(page, false);

      await pressComposerControl(page, WIP_TOGGLE);
      await expect.poll(() => appFrameUrl(page), { timeout: 10_000 })
        .toContain(`thread_id=${seeded.threadId}`);

      await ensureMobileView(page, 'thread');

      await pressComposerControl(page, WIP_TOGGLE);
      await expect.poll(() => appFrameUrl(page), { timeout: 5_000 })
        .not.toContain('thread_id=');
    } finally {
      cleanupCCThread(seeded.threadId, seeded.changeId, seeded.branch, seeded.file);
      fixture.cleanup();
      try { git(['add', `data/apps/${appId}`]); git(['commit', '-m', `e2e cleanup app ${appId}`, '--allow-empty']); } catch {  }
    }
  });
});
