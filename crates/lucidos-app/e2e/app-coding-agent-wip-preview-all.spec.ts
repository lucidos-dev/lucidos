import { test, expect, type Page } from './fixtures';
import {
  navigateToApp, assertHealthy, composerToggleState, ensureMobileView, pressComposerControl,
} from './helpers';
import { createIframeAppFixture, createAppCCThreadWithChange, cleanupCCThread, git } from './db-helpers';

/** Where the app frame currently is, asked of the driver.
 *
 *  An app frame is isolated (ADR 0227), so `contentWindow.location` throws for
 *  the page. Playwright tracks a frame's URL from the protocol and answers
 *  whatever its origin. It also reports where the frame actually IS, which the
 *  `src` attribute stops doing the moment `location.replace` is used. */
function appFrameUrl(page: import('@playwright/test').Page): string {
  return page.frames().map((f) => f.url()).find((u) => u.includes('/app/')) ?? '';
}

const WIP_TOGGLE = '[data-role="wip-preview-toggle"]';

/** Wait for the WIP toggle, standing in the row or behind its ⋯. */
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
      await expectWipToggle(page);

      // Click → the frame navigates to `?thread_id=<id>`. The navigation is a
      // `location.replace`, to avoid session-history pollution (WebKit #9166),
      // so the iframe's `src` attribute never changes and cannot be read for
      // the answer. `appFrameUrl` asks the driver, which an isolated frame
      // does not refuse the way `contentWindow.location` does.
      await pressComposerControl(page, WIP_TOGGLE);
      await expect.poll(() => appFrameUrl(page), { timeout: 5_000 })
        .toContain(`thread_id=${seeded.threadId}`);

      // Click again → reverts to live.
      await pressComposerControl(page, WIP_TOGGLE);
      await expect.poll(() => appFrameUrl(page), { timeout: 5_000 })
        .not.toContain('thread_id=');
    } finally {
      cleanupCCThread(seeded.threadId, seeded.changeId, seeded.branch, seeded.file);
      fixture.cleanup();
      try { git(['add', `data/apps/${appId}`]); git(['commit', '-m', `e2e cleanup app ${appId}`, '--allow-empty']); } catch { /* */ }
    }
  });

  // Regression guard for the "WIP preview is not there" bug: the toggle used to
  // be gated on the target app already being open in the panel-overlay — a
  // chicken-and-egg trap, since the preview is HOW you open the app's WIP. The
  // toggle must be reachable from the thread alone (it has an in-flight diff),
  // and clicking it opens the app in the panel-overlay AND swaps to the WIP URL.
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
    } catch { /* nothing to commit — idempotent */ }

    const seeded = createAppCCThreadWithChange({
      appId,
      titlePrefix: 'WIP preview noopen test',
      suffix,
    });

    try {
      await navigateToApp(page);
      await ensureMobileView(page, 'thread');

      // Focus the seeded coding-agent thread — but DO NOT open the app first.
      await page.evaluate((tid) => {
        location.hash = `thread=${tid}`;
        window.dispatchEvent(new PopStateEvent('popstate'));
      }, seeded.threadId);

      // The toggle is visible from the thread alone (no app open in the panel)
      // and starts in the live (inactive) state.
      await expectWipToggle(page, false);

      // Click → opens the app in the panel-overlay AND points its iframe at the
      // worktree-served WIP URL (`?thread_id=<id>`).
      await pressComposerControl(page, WIP_TOGGLE);
      await expect.poll(() => appFrameUrl(page), { timeout: 10_000 })
        .toContain(`thread_id=${seeded.threadId}`);

      // Turning WIP on opened the app, which on mobile swipes to the content
      // pane (openApp → revealContentPane) — that leaves the toggle, which lives
      // in the thread pane's prompt actions, off-screen. Swipe back to the
      // thread pane before the second click, mirroring the real mobile flow
      // (you return to the thread to toggle the preview off). No-op on desktop.
      await ensureMobileView(page, 'thread');

      // Click again → reverts to live (app stays open, no thread_id).
      await pressComposerControl(page, WIP_TOGGLE);
      await expect.poll(() => appFrameUrl(page), { timeout: 5_000 })
        .not.toContain('thread_id=');
    } finally {
      cleanupCCThread(seeded.threadId, seeded.changeId, seeded.branch, seeded.file);
      fixture.cleanup();
      try { git(['add', `data/apps/${appId}`]); git(['commit', '-m', `e2e cleanup app ${appId}`, '--allow-empty']); } catch { /* */ }
    }
  });
});
