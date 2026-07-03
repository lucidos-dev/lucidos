import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  waitForVisibleInput,
  openThreadDrawer,
} from './helpers';
import { clearAllThreads } from './db-helpers';

// Desktop-only layout test for the threads-header unified Filter control. The
// `.threads-header` (drawer header) only renders on desktop and depends on
// `page.setViewportSize()` actually changing the layout — which mobile-emulated
// projects ignore (they pin the iPhone viewport via `isMobile: true`). Living in
// a `-desktop.spec.ts` file excludes it from those projects
// (`testIgnore: /-desktop\.spec\.ts$/`).

test.describe('Threads-header unified Filter control — desktop layout', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  const sizeAndOpen = async (page: import('@playwright/test').Page) => {
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);
    await openThreadDrawer(page);
    await page.waitForFunction(() => {
      const header = Array.from(document.querySelectorAll('.threads-header'))
        .find((h) => h.getBoundingClientRect().width > 0);
      const title = header?.querySelector('.threads-header-title');
      return !!title && (title as HTMLElement).getBoundingClientRect().width > 0;
    }, undefined, { timeout: 10_000 });
    // The drawer/header width animates for var(--duration-slow) (300ms). Settle
    // before measuring so geometry isn't mixed with the drawer-open transition.
    await page.waitForTimeout(400);
  };

  test('one Filter button, no separate view selector, holds the Threads title in place', async ({ page }) => {
    await sizeAndOpen(page);

    const measure = async () => page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.threads-header'))
        .find((h) => h.getBoundingClientRect().width > 0) as HTMLElement | undefined;
      if (!header) return null;
      const title = header.querySelector('.threads-header-title') as HTMLElement | null;
      const filter = header.querySelector('button[aria-label="Filter threads"]') as HTMLElement | null;
      const selector = header.querySelector('button[aria-label="Switch thread view"]');
      const rect = (el: HTMLElement | null) => el ? el.getBoundingClientRect() : null;
      return {
        titleTextAlign: title ? getComputedStyle(title).textAlign : '',
        titleLeft: rect(title)?.left ?? 0,
        filterWidth: rect(filter)?.width ?? 0,
        filterRight: rect(filter)?.right ?? 0,
        hasSeparateSelector: !!selector,
      };
    });

    const empty = await measure();
    expect(empty, 'visible threads-header').not.toBeNull();
    // The view selector has been merged into the Filter control — there is no
    // separate "Switch thread view" button anymore.
    expect(empty!.hasSeparateSelector, 'no separate view-selector button').toBe(false);
    expect(empty!.filterWidth, 'single Filter button is visible').toBeGreaterThan(20);
    // The Filter button sits left of the Threads title box (the title is flex:1,
    // so its box starts right after the button).
    expect(empty!.filterRight, 'Filter button sits left of the Threads title')
      .toBeLessThanOrEqual(empty!.titleLeft + 1);
    // The title centres in the gap between the Filter button and the Search icon
    // (079672700 — "center Threads title between Filter and Search icons").
    expect(empty!.titleTextAlign, 'Threads title text centres between Filter and Search').toBe('center');

    // The needs-attention badge is absolutely positioned, so even a draft that
    // surfaces per-view counts in the menu must not move the title.
    const input = await waitForVisibleInput(page);
    await input.fill('an unsent draft to surface the drafts count');
    await page.waitForTimeout(100);
    const withDraft = await measure();
    expect(Math.abs(withDraft!.titleLeft - empty!.titleLeft), 'Threads title moved when a draft appeared')
      .toBeLessThan(1);
  });

  test('opens the merged View + Show dropdown; picking a view closes it; non-All view greys the Show section', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    await filterBtn.click();

    const dropdown = page.locator('.threads-header .thread-filter-dropdown');
    await expect(dropdown).toBeVisible();

    // The View section lists all five views in order.
    const labels = await dropdown.locator('.drawer-view-option .drawer-view-label').allTextContents();
    expect(labels).toEqual(['All statuses', 'Needs attention', 'Review', 'Running', 'Drafts']);

    // In the default All view the Show (channel) section is enabled — assert via
    // a channel checkbox (a descendant of the `<fieldset>`; Playwright reports a
    // checkbox in a disabled fieldset as disabled, but not the fieldset itself).
    const firstChannel = dropdown.locator('fieldset.thread-filter-show input[type="checkbox"]').first();
    await expect(firstChannel).toBeEnabled();

    // Picking a view applies it and closes the dropdown (selecting a status is a
    // terminal choice, not a step the user keeps adjusting).
    await dropdown.locator('.drawer-view-option', { hasText: 'Review' }).click();
    await expect(dropdown).toHaveCount(0);

    // Reopening with a non-All view active shows the Show section greyed in place
    // (those views bypass the channel filter — no separate disabled button, no
    // tooltip).
    await filterBtn.click();
    await expect(dropdown).toBeVisible();
    await expect(dropdown.locator('fieldset.thread-filter-show input[type="checkbox"]').first()).toBeDisabled();

    await page.keyboard.press('Escape');
    await expect(dropdown).toHaveCount(0);
  });

  test('the Filter button opens reliably and toggles closed (Chrome open-bug regression)', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const dropdown = page.locator('.threads-header .thread-filter-dropdown');

    // Fresh click opens it (the old separate view selector failed to open here).
    await filterBtn.click();
    await expect(dropdown).toBeVisible();

    // Re-tapping the toggle closes it (anchor exemption — not re-opened by the
    // dismiss-then-toggle race).
    await filterBtn.click();
    await expect(dropdown).toHaveCount(0);

    // And it opens again on the next click.
    await filterBtn.click();
    await expect(dropdown).toBeVisible();
  });
});
