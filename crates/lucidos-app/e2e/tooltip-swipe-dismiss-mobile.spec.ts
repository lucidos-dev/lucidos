/**
 * Mobile: a tap-revealed header-title tooltip dismisses on the next swipe.
 *
 * Regression (the "sticky on swipe to thread view" report): tapping the
 * content-header title revealed the full-title tooltip (`#tooltip`,
 * data-tooltip-tap), but swiping to another pane left it floating over the
 * swiped-away target — useTooltip's onTouchMove flagged the swipe but never hid
 * an already-visible tooltip, and onTouchEnd returns early for a swipe. The fix
 * hides a visible tooltip the moment a swipe is detected (useTooltip.ts).
 *
 * We reproduce the exact screenshot scenario: a long notification title in the
 * content header truncates, so its tooltip is NOT suppressed as redundant. The
 * swipe is dispatched as a real touch sequence so useTooltip's document-level
 * capture handlers run exactly as on a device.
 */
import { test, expect, Page } from './fixtures';
import {
  apiRequest, assertHealthy, clickVisibleElement, ensureMobileView, isMobileViewport,
  navigateToApp, waitForVisibleElement,
} from './helpers';
import { clearNotifications } from './db-helpers';

const LONG_TITLE =
  'Nightly pipeline: Steps 1-4 GREEN; Step 5 (E2E) FAILED on 1 deterministic test';

/** Dispatch a left→right swipe as a synthetic touch sequence on `selector`, far
 *  enough to clear useTooltip's 10px swipe threshold. Uses generic `Event`s with
 *  hand-defined touch lists rather than `new Touch()` / `new TouchEvent()` — the
 *  Touch/TouchEvent constructors are an "Illegal constructor" in WebKit (iOS
 *  Safari), the very engine this regression targets. addEventListener matches on
 *  the event-type string, so a plain Event('touchmove') still fires useTooltip's
 *  document-level 'touchmove' capture listener, and the handler reads only
 *  `.touches[0].clientX/Y`, which the defined properties supply. */
async function swipeOn(page: Page, selector: string): Promise<void> {
  await page.evaluate((sel) => {
    const el = Array.from(document.querySelectorAll<HTMLElement>(sel)).find(
      (e) => e.getBoundingClientRect().width > 0,
    );
    if (!el) throw new Error(`no visible element for ${sel}`);
    const r = el.getBoundingClientRect();
    const y = r.top + r.height / 2;
    const x0 = r.left + r.width / 2;
    const mk = (type: string, x: number) => {
      const ev = new Event(type, { bubbles: true, cancelable: true, composed: true });
      const touch = { identifier: 1, target: el, clientX: x, clientY: y, pageX: x, pageY: y };
      const list = type === 'touchend' ? [] : [touch];
      Object.defineProperty(ev, 'touches', { value: list });
      Object.defineProperty(ev, 'targetTouches', { value: list });
      Object.defineProperty(ev, 'changedTouches', { value: [touch] });
      return ev;
    };
    el.dispatchEvent(mk('touchstart', x0));
    el.dispatchEvent(mk('touchmove', x0 + 60));
    el.dispatchEvent(mk('touchend', x0 + 60));
  }, selector);
}

test.describe('Mobile tooltip dismiss on swipe', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
    test.skip(!isMobileViewport(page), 'Touch swipe behavior only — skipped on desktop project');
  });

  test('tap-revealed content-title tooltip hides when the user swipes away', async ({ page }) => {
    const res = await apiRequest(page).post('/api/v1/notifications', {
      headers: { 'content-type': 'application/json' },
      data: { title: LONG_TITLE, message: 'pipeline body' },
    });
    expect(res.ok(), `POST /api/v1/notifications -> ${res.status()}`).toBeTruthy();

    await navigateToApp(page);
    await ensureMobileView(page, 'content');

    // Open the notifications list (content pane), then the detail — its title
    // becomes the content-header title, truncated at the mobile width.
    await clickVisibleElement(page, '.notifications-bell');
    await waitForVisibleElement(page, '.notification-item', 10_000);
    await clickVisibleElement(page, '.notification-item');
    await waitForVisibleElement(page, '.notification-detail-body', 10_000);

    const title = page.locator('.mobile-content-title:visible').first();
    await expect(title).toBeVisible();
    // The tooltip must have something to add, else it is suppressed as
    // redundant and there is nothing to dismiss. Asserted with the app's own
    // rule (`isRedundantTooltip`), not with truncation alone: the bar now
    // renders the SHORT form of a title we author, so this header reads
    // "Notification" and fits, while the tooltip still carries the
    // notification's own long title. Ellipsis is one way to earn a tooltip,
    // and since `getContentTitleShort` it is no longer the only one.
    const revealable = await title.evaluate((el) => {
      const tip = (el.getAttribute('data-tooltip') ?? '').trim().toLowerCase();
      const visible = (el.textContent ?? '').trim().toLowerCase();
      const truncated = el.scrollWidth > el.clientWidth;
      return !!tip && (truncated || tip !== visible);
    });
    expect(revealable, 'content title must have a tooltip that says more than the bar does').toBeTruthy();

    // Tap to reveal the full-title tooltip.
    await title.tap();
    await expect(page.locator('#tooltip')).toBeVisible({ timeout: 5_000 });

    // Swipe — the tooltip must vanish instead of staying stuck over the header.
    await swipeOn(page, '.mobile-content-title');
    await expect(page.locator('#tooltip')).toBeHidden({ timeout: 5_000 });
  });
});
