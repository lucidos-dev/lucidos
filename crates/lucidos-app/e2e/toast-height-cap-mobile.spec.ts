import { test, expect } from './fixtures';
import { assertHealthy, gotoWithRetry } from './helpers';

const LONG_BODY = [
  'Memory is being squeezed: free 3.67 GB (18.32 GB reclaimable), compressor 11.40 GB, swap 0.08 GB, pressure critical.',
  'Nothing safe to reclaim automatically. Save your work and find the hogs with',
  'ps -Aww -o rss,command | sort -rn | head -20.',
  'Latest: up 2 days | wired 3.80GB | comp 11.40GB | free 3.67GB | press critical | swap 0.08GB | wk 0/0.00GB',
].join(' ');

const MAX_VIEWPORT_FRACTION = 0.45;

test.describe('toast height cap on mobile', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a long body is clamped and scrolls, instead of covering the pane', async ({ page }) => {
    await gotoWithRetry(page, '/');
    await expect(page.locator('.app-header').first()).toBeVisible({ timeout: 15_000 });

    const geom = await page.evaluate((body: string) => {
      const container = document.createElement('div');
      container.className = 'toast-container';
      const column = document.createElement('div');
      column.className = 'toast-column';
      const toast = document.createElement('div');
      toast.className = 'toast surface toast-info';
      toast.innerHTML =
        '<svg class="toast-icon" viewBox="0 0 24 24"></svg>' +
        '<div class="toast-body">' +
        '<div class="toast-heading"></div>' +
        '</div>' +
        '<div class="toast-actions button-group">' +
        '<button class="action-btn">Open</button>' +
        '</div>' +
        '<button class="icon-btn toast-close" aria-label="Dismiss"></button>';
      (toast.querySelector('.toast-heading') as HTMLElement).textContent = body;
      column.appendChild(toast);
      container.appendChild(column);
      document.body.appendChild(container);

      const toastRect = toast.getBoundingClientRect();
      const heading = toast.querySelector('.toast-heading') as HTMLElement;
      const open = toast.querySelector('.toast-actions .action-btn') as HTMLElement;
      const openRect = open.getBoundingClientRect();
      const result = {
        toastHeight: toastRect.height,
        toastBottom: toastRect.bottom,
        viewportHeight: window.innerHeight,
        headingOverflows: heading.scrollHeight > heading.clientHeight + 1,
        headingScrolls: getComputedStyle(heading).overflowY === 'auto',
        headingClientHeight: heading.clientHeight,
        headingLineHeight: parseFloat(getComputedStyle(heading).lineHeight),
        openTop: openRect.top,
        openBottom: openRect.bottom,
        openHeight: openRect.height,
      };
      container.remove();
      return result;
    }, LONG_BODY);

    expect(
      geom.headingOverflows,
      'the probe body did not overflow, so this asserts nothing about the cap',
    ).toBe(true);
    expect(geom.headingScrolls, 'the clamped overflow must scroll, not be clipped away').toBe(true);

    const fraction = geom.toastHeight / geom.viewportHeight;
    expect(
      fraction,
      `a single toast takes ${(fraction * 100).toFixed(0)}% of the ${geom.viewportHeight}px viewport`,
    ).toBeLessThanOrEqual(MAX_VIEWPORT_FRACTION);

    expect(geom.toastBottom).toBeLessThanOrEqual(geom.viewportHeight + 1);
    expect(geom.openHeight).toBeGreaterThan(0);
    expect(geom.openBottom).toBeLessThanOrEqual(geom.toastBottom + 1);
    expect(geom.openTop).toBeGreaterThanOrEqual(0);

    const lines = geom.headingClientHeight / geom.headingLineHeight;
    expect(
      Math.abs(lines - Math.round(lines)),
      `the heading shows ${lines.toFixed(2)} lines, so the last one is sheared`,
    ).toBeLessThan(0.05);
  });
});
