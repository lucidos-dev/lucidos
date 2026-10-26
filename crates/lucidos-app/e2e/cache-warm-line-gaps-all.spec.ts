/**
 * The cache-warm line is a card between the last turn and the composer. The
 * space above it, from the last turn's visible edge, equals the space below
 * it, to the composer box, at phone and desktop width.
 *
 * The card has no top padding. The last turn's bottom padding is the gap
 * above, and the transcript's bottom padding is the gap below. Either one
 * drifting breaks the equality, which is what this measures.
 *
 * The engine's answer is stubbed: the line needs a warm cache, and a real one
 * needs a real model call inside the cache's lifetime.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, sendMessage, uniqueMessage, waitForResponse } from './helpers';

const WARMTH = {
  remaining_secs: 240,
  keep_alive: { state: 'not_kept', reason: 'nothing_pending' },
};

// The service worker answers GETs under /api/v1 itself, so `page.route` never sees them.
test.use({ serviceWorkers: 'block' });

/** A window short enough that one reply overflows the transcript. */
const SHORT_WINDOW_PX = 420;

/** The gap, in px, within which "equal" holds. */
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
    if (!card || !edgeOwner || !box) return null;
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
    await page.route('**/api/v1/threads/*/cache-warmth', (route) => route.fulfill({ json: WARMTH }));
  });

  test('the space above the card equals the space below it', async ({ page }) => {
    await navigateToApp(page);
    await sendMessage(page, `Say exactly: "gaps ${uniqueMessage('warm-gaps')}"`);
    await waitForResponse(page);

    await expect(page.locator('[data-role="cache-warm"]:visible')).toBeVisible({ timeout: 15_000 });
    // Short content leaves the composer far below the card. A short window
    // makes the transcript scroll, and the card ends it.
    const { width } = page.viewportSize()!;
    await page.setViewportSize({ width, height: SHORT_WINDOW_PX });
    await expect.poll(() => scrollTranscriptToEnd(page), { timeout: 10_000 }).toBe(true);
    await expect.poll(() => measureGaps(page), { timeout: 10_000 }).not.toBeNull();
    const gaps = (await measureGaps(page))!;

    expect(gaps.above, 'the card sits below the last turn').toBeGreaterThan(0);
    expect(gaps.below, 'the composer sits below the card').toBeGreaterThan(0);
    expect(Math.abs(gaps.above - gaps.below), `gap above ${gaps.above}px, gap below ${gaps.below}px`)
      .toBeLessThanOrEqual(TOLERANCE_PX);
  });
});
