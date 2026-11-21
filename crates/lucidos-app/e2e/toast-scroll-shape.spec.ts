/**
 * A titled toast keeps its title, and scrolls its message against its own
 * right edge.
 *
 * The report (a build toast listing a dozen commits) was two things at once.
 * The scroll box sat inside the gutter reserved for the close X, so its
 * scrollbar was drawn mid-card with the X beyond it. Its `--bg-primary` track
 * then read as a black slot through the message. And line 1, the count that
 * says what the toast is about, scrolled away with the list.
 *
 * Both are geometry, so they are measured rather than scanned.
 * `styles/__tests__/toast-height-cap.test.ts` pins what CSS can state on its
 * own. What only a browser resolves is where the four boxes actually land,
 * once the cascade, the root font size and the flex shrink have run.
 *
 * The toast is SPLICED into the live page rather than raised through a real
 * build. Same technique, and reason, as `toast-height-cap-mobile.spec.ts`.
 */
import { test, expect } from './fixtures';
import { assertHealthy, gotoWithRetry } from './helpers';

/** The shape of a build toast: a count for the title, and enough bullet lines
 *  in the message to overflow the 14rem cap. */
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
    // The toast container only exists while a toast does, so wait for the shell
    // instead: the probe needs the real --app-header-bottom and theme tokens.
    await expect(page.locator('.app-header').first()).toBeVisible({ timeout: 15_000 });

    const geom = await page.evaluate(
      ({ heading, bullets }: { heading: string; bullets: string[] }) => {
        // Mirrors the markup `renderToast` emits (Toast.tsx) for a titled toast:
        // the icon and the close X are the body's siblings, the title is the
        // heading, and the message is in the box that scrolls.
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
          // Proves there was something to scroll, so the rest asserts anything
          // at all.
          textOverflow: text.scrollHeight > text.clientHeight + 1,
          scrolled: text.scrollTop > 1,
          headingHeld: Math.abs(headingAfter.top - headingBefore.top) < 1,
          headingTextLeft: headingBefore.left + parseFloat(getComputedStyle(headingEl).paddingLeft),
          headingRight: headingBefore.right,
          textTop: textRect.top,
          textLeft: textRect.left,
          textRight: textRect.right,
          // The toast's content-box edges: where the card's own rail is.
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

    // 1. The heading holds. It names what the toast is about, so a scroll to
    //    the last bullet must not take it off the top.
    expect(geom.headingHeld, 'the heading moved when the text scrolled').toBe(true);

    // 2. The scroll box runs to the card's own content edge, which is where its
    //    scrollbar is drawn. Inside that edge is the text column, where the bar
    //    used to sit.
    expect(Math.abs(geom.textRight - geom.contentRight)).toBeLessThan(1);

    // 3. And it starts below the close X, so the bar never runs under the
    //    glyph. The heading's floor is what guarantees the clearance.
    expect(geom.textTop).toBeGreaterThanOrEqual(geom.closeBottom - 1);

    // 4. The heading text starts past the icon gutter and stops short of the
    //    button, since it is the one line beside both.
    expect(geom.headingTextLeft).toBeGreaterThan(geom.contentLeft + geom.iconWidth - 1);
    expect(geom.headingRight).toBeLessThanOrEqual(geom.closeLeft + 1);

    // 5. The text sits below the icon, so it pays no gutter for it.
    expect(Math.abs(geom.textLeft - geom.contentLeft)).toBeLessThan(1);
  });
});
