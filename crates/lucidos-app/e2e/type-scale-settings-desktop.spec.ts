/**
 * Settings > System > What's New is on the type scale.
 *
 * This is the surface the whole text-defaults layer was written for: its
 * release notes shipped LARGER than the version heading above them, because
 * `.whats-new-notes` styled its padding and stopped and the block fell all the
 * way through to the root, which is `--font-size-xl`, a section heading.
 *
 * Desktop-only, and the reason is the viewport rather than the engine. The
 * spec pins 1280x900 and walks a settings panel laid out at that width. The
 * `-desktop` name is what keeps it out of the mobile-webkit project.
 *
 * It is NOT the `waitForEventStream` that `POST /api/v1/ui/navigate` needs.
 * That wait works under `mobile-webkit`, as `scroll-memory`, `thread-queue`
 * and `ui-scale-slider-mobile` all show. The engine-agnostic half of the
 * coverage runs on all three projects in `type-scale.spec.ts`: the defaults
 * themselves, and the thread-view walk. So nothing about the CSS goes
 * unchecked on WebKit.
 */
import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';
import { offenders, report } from './typeScaleWalk';

test.use({ viewport: { width: 1280, height: 900 } });

test.describe('Type scale in Settings', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test("every visible text run in What's New is on the scale", async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);
    const res = await apiRequest(page).post('/api/v1/ui/navigate', {
      headers: { 'content-type': 'application/json' },
      data: { target: 'settings', params: { settings_view: 'whats-new' } },
    });
    expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

    await page.waitForFunction(
      () => {
        const panel = document.querySelector('.settings-panel');
        if (!panel) return false;
        const rect = panel.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      },
      undefined,
      { timeout: 15_000 }
    );

    // Open the first release so the markdown notes are actually mounted: the
    // panel renders each release's notes only when its row is opened, so a
    // collapsed list would walk right past the surface under test.
    const firstRelease = page.locator('.whats-new-row, .whats-new-toggle').first();
    if (await firstRelease.count()) {
      await firstRelease.click();
      await page.waitForTimeout(300);
    }

    const found = await offenders(page);
    expect(found, `off-scale text in What's New:\n${report(found)}`).toEqual([]);
  });
});
