import { test, expect } from './fixtures';
import { assertHealthy, gotoWithRetry } from './helpers';

const HEADING = '12 commits come with the new version';
const BULLETS = [
  'header: the unread total rides the brand and the menu says where it lives',
  'gateway: the pairing screen is not the picker, and owns its own boot',
  'header: every badge on the bar is ringed clear of the glyph it rides',
  'toast: the heading stays and the scrollbar moves to the right rail',
  'picker: the network popover shows what is stored, not what was clicked',
  'drawer: the toggle travels instead of crossfading two copies',
];

test.describe('toast scroll shape', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('holds the heading, and runs the scroll box to the card edge', async ({ page }) => {
    await gotoWithRetry(page, '/');
    await expect(page.locator('.app-header').first()).toBeVisible({ timeout: 15_000 });

    const geom = await page.evaluate(
      ({ heading, bullets }: { heading: string; bullets: string[] }) => {
        const container = document.createElement('div');
        container.className = 'toast-container';
        const column = document.createElement('div');
        column.className = 'toast-column';
        const toast = document.createElement('div');
        toast.className = 'toast surface toast-info';
        toast.innerHTML =
          '<svg class="toast-icon" viewBox="0 0 24 24"></svg>' +
          '<div class="toast-body">' +
          '<div class="toast-heading"><span class="toast-title"></span></div>' +
          '<div class="toast-text"></div>' +
          '</div>' +
          '<button class="icon-btn toast-close" aria-label="Dismiss"></button>';
        (toast.querySelector('.toast-title') as HTMLElement).textContent = heading;
        (toast.querySelector('.toast-text') as HTMLElement).textContent =
          bullets.map((b) => `• ${b}`).join('\n');
        column.appendChild(toast);
        container.appendChild(column);
        document.body.appendChild(container);

        const headingEl = toast.querySelector('.toast-heading') as HTMLElement;
        const text = toast.querySelector('.toast-text') as HTMLElement;
        const close = toast.querySelector('.toast-close') as HTMLElement;
        const style = getComputedStyle(toast);
        const toastRect = toast.getBoundingClientRect();
        const headingBefore = headingEl.getBoundingClientRect();

        text.scrollTop = text.scrollHeight;
        const headingAfter = headingEl.getBoundingClientRect();

        const textRect = text.getBoundingClientRect();
        const closeRect = close.getBoundingClientRect();
        const result = {
          textOverflow: text.scrollHeight > text.clientHeight + 1,
          scrolled: text.scrollTop > 1,
          headingHeld: Math.abs(headingAfter.top - headingBefore.top) < 1,
          headingTextLeft: headingBefore.left + parseFloat(getComputedStyle(headingEl).paddingLeft),
          headingRight: headingBefore.right,
          textTop: textRect.top,
          textLeft: textRect.left,
          textRight: textRect.right,
          contentRight:
            toastRect.right - parseFloat(style.borderRightWidth) - parseFloat(style.paddingRight),
          contentLeft:
            toastRect.left + parseFloat(style.borderLeftWidth) + parseFloat(style.paddingLeft),
          iconWidth: (toast.querySelector('.toast-icon') as HTMLElement).getBoundingClientRect().width,
          closeBottom: closeRect.bottom,
          closeLeft: closeRect.left,
        };
        container.remove();
        return result;
      },
      { heading: HEADING, bullets: BULLETS },
    );

    expect(geom.textOverflow, 'the probe did not overflow, so nothing here is under test').toBe(true);
    expect(geom.scrolled, 'the probe box did not scroll, so the heading test proves nothing').toBe(true);

    expect(geom.headingHeld, 'the heading moved when the text scrolled').toBe(true);

    expect(Math.abs(geom.textRight - geom.contentRight)).toBeLessThan(1);

    expect(geom.textTop).toBeGreaterThanOrEqual(geom.closeBottom - 1);

    expect(geom.headingTextLeft).toBeGreaterThan(geom.contentLeft + geom.iconWidth - 1);
    expect(geom.headingRight).toBeLessThanOrEqual(geom.closeLeft + 1);

    expect(Math.abs(geom.textLeft - geom.contentLeft)).toBeLessThan(1);
  });
});
