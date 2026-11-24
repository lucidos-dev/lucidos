import { test, expect } from './fixtures';
import { apiRequest, navigateToApp, waitForEventStream } from './helpers';


const SUBVIEW = 'models';
const SUBVIEW_SCROLL_KEY = `lucidos-scroll-content-settings:${SUBVIEW}`;

test.describe('scroll position restore', () => {
  test('content pane restores saved scroll after async content renders', async ({ page }) => {
    const currentSize = page.viewportSize();
    if (currentSize) {
      await page.setViewportSize({ width: currentSize.width, height: 320 });
    }
    await page.addInitScript((key: string) => {
      localStorage.setItem(key, '40');
    }, SUBVIEW_SCROLL_KEY);

    await navigateToApp(page);
    await waitForEventStream(page);

    const res = await apiRequest(page).post('/api/v1/ui/navigate', {
      headers: { 'content-type': 'application/json' },
      data: { target: 'settings', params: { settings_view: SUBVIEW } },
    });
    expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

    await page.waitForTimeout(4000);

    const result = await page.evaluate(() => {
      const els = document.querySelectorAll('.content-pane-body');
      for (const el of els) {
        const rect = el.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          const max = Math.max(0, el.scrollHeight - el.clientHeight);
          return { found: true, scrollTop: el.scrollTop, max };
        }
      }
      return { found: false, scrollTop: -1, max: 0 };
    });
    expect(result.found).toBe(true);
    expect(result.max).toBeGreaterThan(0);
    expect(result.scrollTop).toBeGreaterThan(0);
  });
});
