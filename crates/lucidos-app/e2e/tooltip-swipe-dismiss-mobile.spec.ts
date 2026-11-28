import { test, expect, Page } from './fixtures';
import {
  apiRequest, assertHealthy, clickVisibleElement, ensureMobileView, isMobileViewport,
  navigateToApp, waitForVisibleElement,
} from './helpers';
import { clearNotifications } from './db-helpers';

const LONG_TITLE =
  'Nightly pipeline: Steps 1-4 GREEN; Step 5 (E2E) FAILED on 1 deterministic test';

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

    await clickVisibleElement(page, '.notifications-bell');
    await waitForVisibleElement(page, '.notification-item', 10_000);
    await clickVisibleElement(page, '.notification-item');
    await waitForVisibleElement(page, '.notification-detail-body', 10_000);

    const title = page.locator('.mobile-content-title:visible').first();
    await expect(title).toBeVisible();
    const revealable = await title.evaluate((el) => {
      const tip = (el.getAttribute('data-tooltip') ?? '').trim().toLowerCase();
      const visible = (el.textContent ?? '').trim().toLowerCase();
      const truncated = el.scrollWidth > el.clientWidth;
      return !!tip && (truncated || tip !== visible);
    });
    expect(revealable, 'content title must have a tooltip that says more than the bar does').toBeTruthy();

    await title.tap();
    await expect(page.locator('#tooltip')).toBeVisible({ timeout: 5_000 });

    await swipeOn(page, '.mobile-content-title');
    await expect(page.locator('#tooltip')).toBeHidden({ timeout: 5_000 });
  });
});
