import { test, expect, Page } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, sendMessage, uniqueMessage, waitForResponse } from './helpers';
import { psql } from './db-helpers';
import type { CacheWarmth } from '../src/api/client/successor';

const WARMTH: CacheWarmth = {
  warmth: 'warm',
  remaining_secs: 240,
  keep_alive: { state: 'not_kept', reason: 'nothing_pending' },
  ttl: { lifetime: '5m', evidence: 'reported' },
};

test.use({ serviceWorkers: 'block' });

const SHORT_WINDOW_PX = 420;

const TOLERANCE_PX = 1;

interface Gaps {
  above: number;
  below: number;
}

async function scrollTranscriptToEnd(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const scrollers = Array.from(document.querySelectorAll('.thread-content')).filter(
      (el) => el.getBoundingClientRect().width > 0,
    );
    const scroller = scrollers[scrollers.length - 1];
    if (!scroller) return false;
    scroller.scrollTop = scroller.scrollHeight;
    return scroller.scrollHeight > scroller.clientHeight;
  });
}

async function measureGaps(page: Page): Promise<Gaps | null> {
  return page.evaluate(() => {
    const lastVisible = (els: NodeListOf<Element>): Element | null => {
      for (let i = els.length - 1; i >= 0; i--) {
        if (els[i].getBoundingClientRect().width > 0) return els[i];
      }
      return null;
    };
    const card = lastVisible(document.querySelectorAll('[data-role="cache-warm"]'));
    const exchange = lastVisible(document.querySelectorAll('.chat-exchange'));
    const box = lastVisible(document.querySelectorAll('.prompt-box'));
    const edgeOwner = exchange?.lastElementChild;
    const scroller = lastVisible(document.querySelectorAll('.thread-content'));
    const promptArea = lastVisible(document.querySelectorAll('.prompt-area'));
    if (!card || !edgeOwner || !box || !scroller || !promptArea) return null;
    const promptRect = promptArea.getBoundingClientRect();
    const overlaid = getComputedStyle(promptArea).position === 'absolute';
    const settledBottom = overlaid ? promptRect.bottom : promptRect.top;
    if (Math.abs(scroller.getBoundingClientRect().bottom - settledBottom) > 0.01) return null;
    const cardRect = card.getBoundingClientRect();
    const edge = edgeOwner.getBoundingClientRect().bottom - parseFloat(getComputedStyle(edgeOwner).paddingBottom);
    return {
      above: cardRect.top - edge,
      below: box.getBoundingClientRect().top - cardRect.bottom,
    };
  });
}

test.describe('the cache-warm line gaps', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    for (const [key, value] of [['cache_keep_alive_mode', 'on'], ['cache_cards', 'true']]) {
      const res = await apiRequest(page).put(`/api/v1/preferences?key=${key}`, { data: { value } });
      expect(res.ok(), `${key}=${value}`).toBe(true);
    }
    await page.route('**/api/v1/threads/*/cache-warmth', (route) => route.fulfill({ json: WARMTH }));
  });

  test.afterEach(() => {
    psql("DELETE FROM preferences WHERE key IN ('cache_cards', 'cache_keep_alive_mode')");
  });

  test('the space above the card equals the space below it', async ({ page }) => {
    await navigateToApp(page);
    await sendMessage(page, `Say exactly: "gaps ${uniqueMessage('warm-gaps')}"`);
    await waitForResponse(page);

    await expect(page.locator('[data-role="cache-warm"]:visible')).toBeVisible({ timeout: 15_000 });
    const { width } = page.viewportSize()!;
    await page.setViewportSize({ width, height: SHORT_WINDOW_PX });
    await expect.poll(() => scrollTranscriptToEnd(page), { timeout: 10_000 }).toBe(true);
    let gaps: Gaps | null = null;
    await expect
      .poll(async () => (gaps = await measureGaps(page)), { timeout: 10_000 })
      .not.toBeNull();
    const { above, below } = gaps!;

    expect(above, 'the card sits below the last turn').toBeGreaterThan(0);
    expect(below, 'the composer sits below the card').toBeGreaterThan(0);
    expect(Math.abs(above - below), `gap above ${above}px, gap below ${below}px`)
      .toBeLessThanOrEqual(TOLERANCE_PX);
  });
});
